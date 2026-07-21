//! Shared-memory document view for out-of-process plugins.
//!
//! The host owns a memory-mapped file that contains a small, read-only,
//! rkyv-serialized view of the active document. The plugin maps the same file
//! read-only and reads entity/layer data directly from the mapping without
//! copying the full `CadDocument` into its own address space.

use std::fs::OpenOptions;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering};

use acadrust::{CadDocument, EntityType, Handle};
use memmap2::{Mmap, MmapMut};
use rkyv::{check_archived_root, to_bytes, Archive, Deserialize, Serialize};
use serde::{Deserialize as SerdeDeserialize, Serialize as SerdeSerialize};

use crate::host::{DocumentReader, ReaderEntity, ReaderEntityKind, ReaderPoint};

/// Magic number identifying a valid control page.
const CONTROL_MAGIC: u32 = 0x4F_43_53_44; // "OCSD"

/// Size of the control region at the start of the mapping. Must be enough for
/// `ControlPage` and aligned to a typical page boundary so the snapshot segments
/// that follow are naturally aligned for rkyv.
const CONTROL_SIZE: usize = 4096;

/// Information sent to the plugin so it can open the shared mapping.
#[derive(Debug, Clone)]
pub struct DocumentViewInfo {
    /// Absolute path to the memory-mapped file.
    pub path: String,
    /// Snapshot version at the time the view was opened.
    pub version: u64,
}

/// Host-side, file-backed double buffer for the document view.
pub struct DocumentSnapshotStore {
    path: PathBuf,
    mmap: MmapMut,
    segment_size: usize,
    current_version: u64,
}

impl DocumentSnapshotStore {
    /// Create a new store for `tab`. `segment_size` is the maximum size of one
    /// snapshot buffer; the file is sized to hold two segments plus the control
    /// page.
    pub fn new(tab: usize, segment_size: usize) -> io::Result<Self> {
        let segment_size = segment_size.next_multiple_of(4096);
        static STORE_ID: AtomicUsize = AtomicUsize::new(0);
        let id = STORE_ID.fetch_add(1, Ordering::Relaxed);
        let path = Self::temp_path(tab, id);
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)?;
        let total = CONTROL_SIZE + 2 * segment_size;
        file.set_len(total as u64)?;

        let mut mmap = unsafe { MmapMut::map_mut(&file)? };
        let control = ControlPage::from_bytes_mut(&mut mmap);
        control.magic.store(CONTROL_MAGIC, Ordering::Relaxed);
        control.version.store(0, Ordering::Relaxed);
        control.active_segment.store(0, Ordering::Relaxed);
        control.active_len.store(0, Ordering::Relaxed);
        mmap.flush()?;

        Ok(Self {
            path,
            mmap,
            segment_size,
            current_version: 0,
        })
    }

    fn temp_path(tab: usize, id: usize) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "ocs_plugin_doc_{}_{}_{}_{}.bin",
            std::process::id(),
            tab,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis(),
            id,
        ));
        path
    }

    /// Path the plugin should open to access the mapping.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Serialize `doc` into the inactive segment and atomically publish it.
    pub fn publish(&mut self, doc: &CadDocument) -> io::Result<()> {
        let data = DocumentViewData::from(doc);
        let bytes = to_bytes::<_, 256>(&data).map_err(|e| {
            io::Error::new(io::ErrorKind::InvalidData, format!("rkyv serialize: {e}"))
        })?;
        if bytes.len() > self.segment_size {
            return Err(io::Error::new(
                io::ErrorKind::OutOfMemory,
                format!(
                    "document view {} bytes exceeds segment size {}",
                    bytes.len(),
                    self.segment_size
                ),
            ));
        }

        let inactive = {
            let control = ControlPage::from_bytes_mut(&mut self.mmap);
            let active = control.active_segment.load(Ordering::Acquire) as usize;
            1 - active
        };
        let offset = CONTROL_SIZE + inactive * self.segment_size;

        self.mmap[offset..offset + bytes.len()].copy_from_slice(&bytes);
        let control = ControlPage::from_bytes_mut(&mut self.mmap);
        // Ensure the plugin sees the new length before it sees the new version.
        control
            .active_len
            .store(bytes.len() as u64, Ordering::Release);
        control
            .active_segment
            .store(inactive as u8, Ordering::Release);
        self.current_version = self.current_version.wrapping_add(1);
        control
            .version
            .store(self.current_version, Ordering::Release);
        self.mmap.flush()?;
        Ok(())
    }

    /// Current published version.
    pub fn version(&self) -> u64 {
        ControlPage::from_bytes(&self.mmap)
            .version
            .load(Ordering::Acquire)
    }
}

impl Drop for DocumentSnapshotStore {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Plugin-side read-only mapping of the host's document view.
pub struct SharedDocumentReader {
    mmap: Mmap,
    segment_size: usize,
    cached_version: u64,
}

impl SharedDocumentReader {
    /// Open the file at `path` read-only and map it. The mapping may initially
    /// contain no valid snapshot; the caller should `refresh()` before use.
    pub fn open(path: &Path) -> io::Result<Self> {
        let file = OpenOptions::new().read(true).open(path)?;
        let mmap = unsafe { Mmap::map(&file)? };
        let file_len = mmap.len();
        let segment_size = if file_len > CONTROL_SIZE {
            (file_len - CONTROL_SIZE) / 2
        } else {
            0
        };
        Ok(Self {
            mmap,
            segment_size,
            cached_version: 0,
        })
    }

    /// Check whether the host has published a newer snapshot.
    pub fn has_new_version(&self) -> bool {
        let control = ControlPage::from_bytes(&self.mmap);
        if control.magic.load(Ordering::Acquire) != CONTROL_MAGIC {
            return false;
        }
        control.version.load(Ordering::Acquire) != self.cached_version
    }

    /// Update the cached version after the caller has re-bound to a new snapshot.
    pub fn refresh(&mut self) {
        let control = ControlPage::from_bytes(&self.mmap);
        self.cached_version = control.version.load(Ordering::Acquire);
    }

    fn active_segment_bytes(&self) -> &[u8] {
        let control = ControlPage::from_bytes(&self.mmap);
        let active = control.active_segment.load(Ordering::Acquire) as usize;
        let len = control.active_len.load(Ordering::Acquire) as usize;
        let offset = CONTROL_SIZE + active * self.segment_size;
        if offset + len > self.mmap.len() {
            return &[];
        }
        &self.mmap[offset..offset + len]
    }

    fn archived(&self) -> Option<&ArchivedDocumentViewData> {
        let bytes = self.active_segment_bytes();
        check_archived_root::<DocumentViewData>(bytes).ok()
    }
}

impl DocumentReader for SharedDocumentReader {
    fn entity_count(&self) -> usize {
        self.archived().map(|doc| doc.entities.len()).unwrap_or(0)
    }

    fn for_each_entity(&self, f: &mut dyn FnMut(ReaderEntity<'_>)) {
        let Some(doc) = self.archived() else { return };
        for entity in doc.entities.iter() {
            let handle = Handle::new(entity.handle);
            let kind = ReaderEntityKind::from_u8(entity.kind);
            let layer_name: &str = entity.layer_name.as_str();
            let point = entity.point.as_ref().map(|p| ReaderPoint {
                x: p.x,
                y: p.y,
                z: p.z,
            });
            f(ReaderEntity {
                handle,
                kind,
                layer_name,
                point,
            });
        }
    }

    fn layer_name(&self, handle: Handle) -> Option<&str> {
        let doc = self.archived()?;
        let handle_val = handle.value();
        doc.layers
            .iter()
            .find(|layer| layer.handle == handle_val)
            .map(|layer| layer.name.as_str())
    }

    fn app_id_name(&self, handle: Handle) -> Option<&str> {
        let doc = self.archived()?;
        let handle_val = handle.value();
        doc.app_ids
            .iter()
            .find(|app| app.handle == handle_val)
            .map(|app| app.name.as_str())
    }
}

/// Raw control page shared between host and plugin.
#[repr(C, align(8))]
struct ControlPage {
    magic: AtomicU32,
    _pad0: [u8; 4],
    version: AtomicU64,
    active_len: AtomicU64,
    active_segment: AtomicU8,
    _pad1: [u8; 7],
}

impl ControlPage {
    fn from_bytes(mmap: &[u8]) -> &Self {
        assert!(mmap.len() >= std::mem::size_of::<Self>());
        assert_eq!(mmap.as_ptr() as usize % std::mem::align_of::<Self>(), 0);
        unsafe { &*(mmap.as_ptr() as *const Self) }
    }

    fn from_bytes_mut(mmap: &mut [u8]) -> &mut Self {
        assert!(mmap.len() >= std::mem::size_of::<Self>());
        assert_eq!(mmap.as_ptr() as usize % std::mem::align_of::<Self>(), 0);
        unsafe { &mut *(mmap.as_ptr() as *mut Self) }
    }
}

/// Serializable document view. This is the only data type placed in shared
/// memory, so it must contain no pointers into host memory.
#[derive(Archive, Serialize, Deserialize, Debug, Clone)]
#[archive(check_bytes)]
pub struct DocumentViewData {
    pub layers: Vec<LayerView>,
    pub app_ids: Vec<AppIdView>,
    pub entities: Vec<EntityView>,
}

impl From<&CadDocument> for DocumentViewData {
    fn from(doc: &CadDocument) -> Self {
        Self {
            layers: doc.layers.iter().map(LayerView::from).collect(),
            app_ids: doc.app_ids.iter().map(AppIdView::from).collect(),
            entities: doc.entities().map(EntityView::from).collect(),
        }
    }
}

#[derive(Archive, Serialize, Deserialize, Debug, Clone)]
#[archive(check_bytes)]
pub struct LayerView {
    pub handle: u64,
    pub name: String,
}

impl From<&acadrust::tables::Layer> for LayerView {
    fn from(layer: &acadrust::tables::Layer) -> Self {
        Self {
            handle: layer.handle.value(),
            name: layer.name.clone(),
        }
    }
}

#[derive(Archive, Serialize, Deserialize, Debug, Clone)]
#[archive(check_bytes)]
pub struct AppIdView {
    pub handle: u64,
    pub name: String,
}

impl From<&acadrust::tables::AppId> for AppIdView {
    fn from(app_id: &acadrust::tables::AppId) -> Self {
        Self {
            handle: app_id.handle.value(),
            name: app_id.name.clone(),
        }
    }
}

#[derive(Archive, Serialize, Deserialize, Debug, Clone)]
#[archive(check_bytes)]
pub struct EntityView {
    pub handle: u64,
    pub kind: u8,
    pub layer_name: String,
    pub point: Option<PointView>,
}

impl From<&EntityType> for EntityView {
    fn from(entity: &EntityType) -> Self {
        let handle = entity.common().handle.value();
        let kind = ReaderEntityKind::from_entity(entity).to_u8();
        let layer_name = entity.common().layer.clone();
        let point = match entity {
            EntityType::Point(p) => Some(PointView {
                x: p.location.x,
                y: p.location.y,
                z: p.location.z,
            }),
            _ => None,
        };
        Self {
            handle,
            kind,
            layer_name,
            point,
        }
    }
}

#[derive(Archive, Serialize, Deserialize, Debug, Clone, Copy)]
#[archive(check_bytes)]
pub struct PointView {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl ReaderEntityKind {
    /// Convert the simplified kind to a stable `u8` for the shared format.
    pub fn to_u8(self) -> u8 {
        match self {
            ReaderEntityKind::Point => 1,
            ReaderEntityKind::Line => 2,
            ReaderEntityKind::Circle => 3,
            ReaderEntityKind::Arc => 4,
            ReaderEntityKind::Polyline => 5,
            ReaderEntityKind::Text => 6,
            ReaderEntityKind::Other => 0,
        }
    }

    /// Decode a stable `u8` back to the simplified kind.
    pub fn from_u8(value: u8) -> Self {
        match value {
            1 => ReaderEntityKind::Point,
            2 => ReaderEntityKind::Line,
            3 => ReaderEntityKind::Circle,
            4 => ReaderEntityKind::Arc,
            5 => ReaderEntityKind::Polyline,
            6 => ReaderEntityKind::Text,
            _ => ReaderEntityKind::Other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::{DocumentReader, ReaderEntityKind};
    use acadrust::entities::Point;
    use acadrust::tables::Layer;
    use acadrust::{CadDocument, EntityType};

    fn sample_doc() -> CadDocument {
        let mut doc = CadDocument::new();
        doc.layers.add(Layer::new("SURVEY")).unwrap();
        let mut point = Point::from_coords(10.0, 20.0, 5.0);
        point.common.layer = "SURVEY".to_string();
        doc.add_entity(EntityType::Point(point)).unwrap();
        doc
    }

    #[test]
    fn shared_document_reader_roundtrip() {
        let doc = sample_doc();
        let mut store = DocumentSnapshotStore::new(0, 1024 * 1024).unwrap();
        store.publish(&doc).unwrap();

        let reader = SharedDocumentReader::open(store.path()).unwrap();
        assert_eq!(reader.entity_count(), 1);

        let mut seen = Vec::new();
        reader.for_each_entity(&mut |e| {
            seen.push((e.kind, e.layer_name.to_string(), e.point, e.handle));
        });
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].0, ReaderEntityKind::Point);
        assert_eq!(seen[0].1, "SURVEY");
        assert_eq!(
            seen[0].2,
            Some(ReaderPoint {
                x: 10.0,
                y: 20.0,
                z: 5.0
            })
        );
        assert!(
            seen[0].3.is_valid(),
            "reader entity should expose a valid handle"
        );
    }

    #[test]
    fn shared_document_reader_updates_after_publish() {
        let doc = sample_doc();
        let mut store = DocumentSnapshotStore::new(0, 1024 * 1024).unwrap();
        store.publish(&doc).unwrap();

        let reader = SharedDocumentReader::open(store.path()).unwrap();
        assert_eq!(reader.entity_count(), 1);

        let mut doc2 = doc;
        let mut point2 = Point::from_coords(1.0, 2.0, 3.0);
        point2.common.layer = "SURVEY".to_string();
        doc2.add_entity(EntityType::Point(point2)).unwrap();
        store.publish(&doc2).unwrap();

        assert_eq!(reader.entity_count(), 2);
    }

    #[test]
    fn layer_name_lookup_by_handle() {
        let doc = sample_doc();
        let mut store = DocumentSnapshotStore::new(0, 1024 * 1024).unwrap();
        store.publish(&doc).unwrap();

        let survey = doc.layers.iter().find(|l| l.name == "SURVEY").unwrap();
        let reader = SharedDocumentReader::open(store.path()).unwrap();
        assert_eq!(reader.layer_name(survey.handle), Some("SURVEY"));
    }
}

// =============================================================================
// Full document snapshot (serde + bincode) and mutation queue
// =============================================================================

/// One entity operation staged by the Python side and applied by the host.
#[derive(Debug, Clone, SerdeSerialize, SerdeDeserialize)]
pub enum EntityOp {
    Add(EntityType),
    Update(EntityType),
    Remove(Handle),
}

// ── Full document snapshot ─────────────────────────────────────────────────

/// Magic number identifying a valid full-snapshot control page.
const FULL_MAGIC: u32 = 0x4F_43_53_46; // "OCSF"

/// Default capacity for a full document snapshot file. Most documents fit
/// comfortably; the file grows if a larger snapshot is needed.
const FULL_DEFAULT_CAPACITY: usize = 64 * 1024 * 1024; // 64 MB

/// Growth increment for the snapshot file.
const FULL_GROWTH_INCREMENT: usize = 64 * 1024 * 1024; // 64 MB

/// Information sent to the Python side so it can open the full snapshot.
#[derive(Debug, Clone)]
pub struct DocumentFullSnapshotInfo {
    pub path: String,
    pub version: u64,
}

/// Host-side, file-backed store for a full `serde` + `bincode` document
/// snapshot. The file is opened only while writing so readers in other
/// processes can open it concurrently.
pub struct DocumentFullSnapshotStore {
    path: PathBuf,
    capacity: usize, // bytes available after the control page
    current_version: u64,
}

impl DocumentFullSnapshotStore {
    /// Create a new store for `tab`. The backing file starts at
    /// `FULL_DEFAULT_CAPACITY` plus the control page and grows on demand.
    pub fn new(tab: usize) -> io::Result<Self> {
        let path = Self::temp_path(tab);
        let capacity = FULL_DEFAULT_CAPACITY;
        let mut file = Self::create_file(&path, capacity)?;
        Self::init_control(&mut file)?;
        Ok(Self {
            path,
            capacity,
            current_version: 0,
        })
    }

    fn create_file(path: &Path, capacity: usize) -> io::Result<std::fs::File> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)?;
        file.set_len((CONTROL_SIZE + capacity) as u64)?;
        Ok(file)
    }

    fn init_control(file: &mut std::fs::File) -> io::Result<()> {
        let mut buf = [0u8; CONTROL_SIZE];
        buf[0..4].copy_from_slice(&FULL_MAGIC.to_le_bytes());
        file.seek(SeekFrom::Start(0))?;
        file.write_all(&buf)?;
        file.flush()?;
        Ok(())
    }

    fn temp_path(tab: usize) -> PathBuf {
        static STORE_ID: AtomicUsize = AtomicUsize::new(0);
        let id = STORE_ID.fetch_add(1, Ordering::Relaxed);
        let mut path = std::env::temp_dir();
        path.push(format!(
            "ocs_plugin_full_doc_{}_{}_{}_{}.bin",
            std::process::id(),
            tab,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis(),
            id,
        ));
        path
    }

    /// Path the Python side should open to access the file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Serialize `doc` into the file and atomically publish a new version.
    /// Grows the backing file if the serialized data does not fit.
    pub fn publish(&mut self, doc: &CadDocument) -> io::Result<()> {
        let bytes = bincode::serialize(doc).map_err(|e| {
            io::Error::new(io::ErrorKind::InvalidData, format!("bincode serialize: {e}"))
        })?;

        if bytes.len() > self.capacity {
            let required = bytes.len().next_multiple_of(FULL_GROWTH_INCREMENT);
            self.grow(required)?;
        }

        let mut file = OpenOptions::new().read(true).write(true).open(&self.path)?;
        // Write the serialized document after the control page.
        file.seek(SeekFrom::Start(CONTROL_SIZE as u64))?;
        file.write_all(&bytes)?;
        file.flush()?;

        // Update the control page last so readers see a consistent version.
        self.current_version = self.current_version.wrapping_add(1);
        write_full_control(
            &mut file,
            self.current_version,
            bytes.len() as u64,
            self.capacity as u64,
        )?;
        Ok(())
    }

    fn grow(&mut self, min_capacity: usize) -> io::Result<()> {
        let new_capacity = min_capacity.max(self.capacity + FULL_GROWTH_INCREMENT);
        let file = OpenOptions::new().read(true).write(true).open(&self.path)?;
        file.set_len((CONTROL_SIZE + new_capacity) as u64)?;
        self.capacity = new_capacity;
        Ok(())
    }

    /// Current published version.
    pub fn version(&self) -> u64 {
        self.current_version
    }
}

impl Drop for DocumentFullSnapshotStore {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn read_full_control(file: &mut std::fs::File) -> io::Result<(u64, u64, u64)> {
    let mut buf = [0u8; CONTROL_SIZE];
    file.read_exact(&mut buf)?;
    let magic = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
    if magic != FULL_MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "full snapshot control page has bad magic",
        ));
    }
    let version = u64::from_le_bytes(buf[8..16].try_into().unwrap());
    let data_len = u64::from_le_bytes(buf[16..24].try_into().unwrap());
    let capacity = u64::from_le_bytes(buf[24..32].try_into().unwrap());
    Ok((version, data_len, capacity))
}

fn write_full_control(
    file: &mut std::fs::File,
    version: u64,
    data_len: u64,
    capacity: u64,
) -> io::Result<()> {
    let mut buf = [0u8; CONTROL_SIZE];
    buf[0..4].copy_from_slice(&FULL_MAGIC.to_le_bytes());
    buf[8..16].copy_from_slice(&version.to_le_bytes());
    buf[16..24].copy_from_slice(&data_len.to_le_bytes());
    buf[24..32].copy_from_slice(&capacity.to_le_bytes());
    file.seek(SeekFrom::Start(0))?;
    file.write_all(&buf)?;
    file.flush()?;
    Ok(())
}

fn read_full_control_file(path: &Path) -> io::Result<(u64, u64, u64)> {
    let mut file = OpenOptions::new().read(true).open(path)?;
    read_full_control(&mut file)
}

/// Python-side read-only view of the host's full document snapshot.
pub struct DocumentFullSnapshotReader {
    path: PathBuf,
    cached_version: u64,
}

impl DocumentFullSnapshotReader {
    /// Open the file at `path`. The file is read on each access so the Python
    /// side always sees the host's latest published snapshot.
    pub fn open(path: &Path) -> io::Result<Self> {
        Ok(Self {
            path: path.to_path_buf(),
            cached_version: 0,
        })
    }

    /// Check whether the host has published a newer snapshot.
    pub fn has_new_version(&self) -> bool {
        read_full_control_file(&self.path)
            .map(|(version, _, _)| version != self.cached_version)
            .unwrap_or(false)
    }

    /// Return the current version.
    pub fn version(&self) -> u64 {
        read_full_control_file(&self.path).map(|(v, _, _)| v).unwrap_or(0)
    }

    /// Deserialize the active snapshot. Returns the document and the version
    /// it corresponds to.
    pub fn refresh(&mut self) -> io::Result<(CadDocument, u64)> {
        let mut file = OpenOptions::new().read(true).open(&self.path)?;
        let (version, data_len, _) = read_full_control(&mut file)?;
        let mut data = vec![0u8; data_len as usize];
        file.read_exact(&mut data)?;
        let doc = bincode::deserialize(&data).map_err(|e| {
            io::Error::new(io::ErrorKind::InvalidData, format!("bincode deserialize: {e}"))
        })?;
        self.cached_version = version;
        Ok((doc, version))
    }
}

// ── Mutation queue ─────────────────────────────────────────────────────────

/// Magic number identifying a valid mutation queue control page.
const QUEUE_MAGIC: u32 = 0x4F_43_53_51; // "OCSQ"

/// Default number of slots in the mutation queue. At 1,000 operations this is
/// far from backpressure; it supports 100,000+ operations in a single batch.
pub const DEFAULT_QUEUE_SLOT_COUNT: usize = 200_000;

/// Default size of one queue slot in bytes. Must be large enough for a
/// serialized `EntityType` with modest metadata; records that do not fit are
/// rejected with [`QueueError::RecordTooLarge`].
pub const DEFAULT_QUEUE_SLOT_SIZE: usize = 1024;

/// Minimum slot size that can hold the header and a tiny payload.
const MIN_SLOT_SIZE: usize = 16;

/// Information sent to the Python side so it can open the mutation queue.
#[derive(Debug, Clone)]
pub struct DocumentMutationQueueInfo {
    pub path: String,
}

/// Errors returned by the mutation queue writer.
#[derive(Debug, Clone, thiserror::Error)]
pub enum QueueError {
    #[error("mutation queue is full")]
    QueueFull,
    #[error("record too large for queue slot")]
    RecordTooLarge,
    #[error("IO error: {0}")]
    Io(String),
}

impl From<io::Error> for QueueError {
    fn from(e: io::Error) -> Self {
        QueueError::Io(e.to_string())
    }
}

impl From<bincode::Error> for QueueError {
    fn from(e: bincode::Error) -> Self {
        QueueError::Io(e.to_string())
    }
}

#[repr(C, align(8))]
struct QueueControlPage {
    magic: AtomicU32,
    _pad0: [u8; 4],
    version: AtomicU64,
    head: AtomicU64,
    tail: AtomicU64,
    slot_count: AtomicU64,
    slot_size: AtomicU32,
    _pad1: [u8; 12],
}

impl QueueControlPage {
    fn from_bytes(mmap: &[u8]) -> &Self {
        assert!(mmap.len() >= std::mem::size_of::<Self>());
        assert_eq!(mmap.as_ptr() as usize % std::mem::align_of::<Self>(), 0);
        unsafe { &*(mmap.as_ptr() as *const Self) }
    }

    fn from_bytes_mut(mmap: &mut [u8]) -> &mut Self {
        assert!(mmap.len() >= std::mem::size_of::<Self>());
        assert_eq!(mmap.as_ptr() as usize % std::mem::align_of::<Self>(), 0);
        unsafe { &mut *(mmap.as_ptr() as *mut Self) }
    }
}

/// Host-side mutation queue: file-backed, shared with the Python process.
pub struct DocumentMutationQueue {
    path: PathBuf,
    mmap: MmapMut,
}

impl DocumentMutationQueue {
    /// Create a new queue for `tab`. Defaults are tuned for the 1,000-point
    /// roundtrip benchmark.
    pub fn new(tab: usize) -> io::Result<Self> {
        Self::with_capacity(tab, DEFAULT_QUEUE_SLOT_COUNT, DEFAULT_QUEUE_SLOT_SIZE)
    }

    /// Create a queue with custom slot count and slot size.
    pub fn with_capacity(
        tab: usize,
        slot_count: usize,
        slot_size: usize,
    ) -> io::Result<Self> {
        let slot_count = slot_count.max(1);
        let slot_size = slot_size.max(MIN_SLOT_SIZE).next_multiple_of(8);
        let path = Self::temp_path(tab);
        let data_size = slot_count * slot_size;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)?;
        file.set_len((CONTROL_SIZE + data_size) as u64)?;
        let mut mmap = unsafe { MmapMut::map_mut(&file)? };
        let control = QueueControlPage::from_bytes_mut(&mut mmap);
        control.magic.store(QUEUE_MAGIC, Ordering::Relaxed);
        control.version.store(0, Ordering::Relaxed);
        control.head.store(0, Ordering::Relaxed);
        control.tail.store(0, Ordering::Relaxed);
        control.slot_count.store(slot_count as u64, Ordering::Relaxed);
        control.slot_size.store(slot_size as u32, Ordering::Relaxed);
        mmap.flush()?;
        Ok(Self {
            path,
            mmap,
        })
    }

    fn temp_path(tab: usize) -> PathBuf {
        static STORE_ID: AtomicUsize = AtomicUsize::new(0);
        let id = STORE_ID.fetch_add(1, Ordering::Relaxed);
        let mut path = std::env::temp_dir();
        path.push(format!(
            "ocs_plugin_mut_q_{}_{}_{}_{}.bin",
            std::process::id(),
            tab,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis(),
            id,
        ));
        path
    }

    /// Path the Python side should open to access the queue.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Drain all ready records from the queue and reset their slots. Invalid
    /// records are reported via `on_error` but do not stop the drain.
    pub fn drain<F>(&mut self, mut on_error: F) -> Vec<EntityOp>
    where
        F: FnMut(&str),
    {
        let mut ops = Vec::new();
        let control = QueueControlPage::from_bytes(&self.mmap);
        let slot_count = control.slot_count.load(Ordering::Acquire) as usize;
        let slot_size = control.slot_size.load(Ordering::Acquire) as usize;
        let tail = control.tail.load(Ordering::Acquire);
        let mut head = control.head.load(Ordering::Acquire);
        while head < tail {
            let idx = (head as usize) % slot_count;
            let slot_base = CONTROL_SIZE + idx * slot_size;
            let state = self.slot_state(slot_base);
            match state.load(Ordering::Acquire) {
                2 => {
                    let len = u32::from_le_bytes([
                        self.mmap[slot_base + 4],
                        self.mmap[slot_base + 5],
                        self.mmap[slot_base + 6],
                        self.mmap[slot_base + 7],
                    ]) as usize;
                    let payload_end = slot_base + 8 + len;
                    if payload_end > self.mmap.len() || len > slot_size - 8 {
                        on_error(&format!("mutation queue slot {idx} has invalid length {len}"));
                    } else {
                        let bytes = &self.mmap[slot_base + 8..payload_end];
                        match bincode::deserialize::<EntityOp>(bytes) {
                            Ok(op) => ops.push(op),
                            Err(e) => on_error(&format!("mutation queue slot {idx} deserialize: {e}")),
                        }
                    }
                    state.store(0, Ordering::Release);
                }
                1 => {
                    // Writer is still in the middle of this slot; stop here
                    // without advancing head so the next drain picks it up.
                    break;
                }
                other => {
                    on_error(&format!("mutation queue slot {idx} has unexpected state {other}"));
                    // Reset to empty and advance to avoid getting stuck.
                    state.store(0, Ordering::Release);
                }
            }
            head = head.wrapping_add(1);
            let control = QueueControlPage::from_bytes_mut(&mut self.mmap);
            control.head.store(head, Ordering::Release);
        }
        ops
    }

    fn slot_state(&self, slot_base: usize) -> &AtomicU32 {
        assert!(slot_base + 4 <= self.mmap.len());
        assert_eq!(self.mmap.as_ptr() as usize % std::mem::align_of::<AtomicU32>(), 0);
        unsafe { &*(self.mmap.as_ptr().add(slot_base) as *const AtomicU32) }
    }
}

impl Drop for DocumentMutationQueue {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Python-side writer for the mutation queue.
pub struct DocumentMutationView {
    mmap: MmapMut,
    slot_size: usize,
}

impl DocumentMutationView {
    /// Open the file at `path` read-write so the queue can be modified.
    pub fn open(path: &Path) -> io::Result<Self> {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        let mmap = unsafe { MmapMut::map_mut(&file)? };
        let control = QueueControlPage::from_bytes(&mmap);
        if control.magic.load(Ordering::Acquire) != QUEUE_MAGIC {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "mutation queue control page has bad magic",
            ));
        }
        let slot_size = control.slot_size.load(Ordering::Acquire) as usize;
        Ok(Self {
            mmap,
            slot_size,
        })
    }

    /// Push a single operation. Returns `Ok(true)` if queued, `Ok(false)` if
    /// the queue is full, or an error if the record is too large.
    pub fn push(&mut self, op: &EntityOp) -> Result<bool, QueueError> {
        let bytes = bincode::serialize(op)?;
        if bytes.len() > self.slot_size - 8 {
            return Err(QueueError::RecordTooLarge);
        }
        let (slot_count, tail, head) = {
            let control = QueueControlPage::from_bytes(&self.mmap);
            (
                control.slot_count.load(Ordering::Acquire) as usize,
                control.tail.load(Ordering::Acquire),
                control.head.load(Ordering::Acquire),
            )
        };
        if tail.wrapping_sub(head) >= slot_count as u64 {
            return Ok(false);
        }
        let idx = (tail as usize) % slot_count;
        let slot_base = CONTROL_SIZE + idx * self.slot_size;
        // Acquire the slot by CAS-ing its state from empty to writing.
        {
            let state = self.slot_state(slot_base);
            loop {
                match state.load(Ordering::Acquire) {
                    0 => {
                        if state
                            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
                            .is_ok()
                        {
                            break;
                        }
                    }
                    1 | 2 => {
                        // Slot still in use; yield and retry.
                        std::thread::yield_now();
                    }
                    _ => {
                        // Corrupt state; try to claim it.
                        let current = state.load(Ordering::Acquire);
                        if state
                            .compare_exchange_weak(current, 1, Ordering::AcqRel, Ordering::Acquire)
                            .is_ok()
                        {
                            break;
                        }
                    }
                }
            }
        }
        // Write length + payload, then publish by setting state = 2.
        self.mmap[slot_base + 4..slot_base + 8]
            .copy_from_slice(&(bytes.len() as u32).to_le_bytes());
        self.mmap[slot_base + 8..slot_base + 8 + bytes.len()].copy_from_slice(&bytes);
        {
            let state = self.slot_state(slot_base);
            state.store(2, Ordering::Release);
        }
        let control = QueueControlPage::from_bytes_mut(&mut self.mmap);
        control.tail.store(tail.wrapping_add(1), Ordering::Release);
        Ok(true)
    }

    /// Push many operations. Returns the number queued; if the queue fills up,
    /// the remainder is left unqueued.
    pub fn push_many(&mut self, ops: impl Iterator<Item = EntityOp>) -> Result<usize, QueueError> {
        let mut queued = 0usize;
        for op in ops {
            match self.push(&op) {
                Ok(true) => queued += 1,
                Ok(false) => break,
                Err(e) => return Err(e),
            }
        }
        Ok(queued)
    }

    fn slot_state(&self, slot_base: usize) -> &AtomicU32 {
        assert!(slot_base + 4 <= self.mmap.len());
        assert_eq!(self.mmap.as_ptr() as usize % std::mem::align_of::<AtomicU32>(), 0);
        unsafe { &*(self.mmap.as_ptr().add(slot_base) as *const AtomicU32) }
    }
}

/// Editor that can apply a batch of entity operations to a host document.
/// Host implementations update their own caches/indices while the generic
/// [`apply_entity_batch`] drives the loop.
pub trait EntityBatchEditor {
    /// Add `entity` and return its assigned handle.
    fn add_entity(&mut self, entity: EntityType) -> Handle;
    /// Replace the entity carrying `entity`'s handle. Returns `false` when the
    /// handle is not present.
    fn update_entity(&mut self, entity: EntityType) -> bool;
    /// Remove the entity with `handle`. Returns it when one existed.
    fn remove_entity(&mut self, handle: Handle) -> Option<EntityType>;
}

/// Apply a sequence of [`EntityOp`]s to `editor`. Returns `(applied, failed)`.
pub fn apply_entity_batch(editor: &mut dyn EntityBatchEditor, ops: Vec<EntityOp>) -> (usize, usize) {
    let mut applied = 0usize;
    let mut failed = 0usize;
    for op in ops {
        match op {
            EntityOp::Add(entity) => {
                let handle = editor.add_entity(entity);
                if handle == Handle::NULL {
                    failed += 1;
                } else {
                    applied += 1;
                }
            }
            EntityOp::Update(entity) => {
                if editor.update_entity(entity) {
                    applied += 1;
                } else {
                    failed += 1;
                }
            }
            EntityOp::Remove(handle) => {
                if editor.remove_entity(handle).is_some() {
                    applied += 1;
                } else {
                    failed += 1;
                }
            }
        }
    }
    (applied, failed)
}

/// Host-side container for the two shared-memory resources used by the
/// Python REPL data path. Keeping this in a single struct makes it easy to
/// store one value per document tab.
pub struct DocumentShmResources {
    full_snapshot: Option<DocumentFullSnapshotStore>,
    mutation_queue: Option<DocumentMutationQueue>,
    tab_index: usize,
}

impl DocumentShmResources {
    pub fn new(tab_index: usize) -> Self {
        Self {
            full_snapshot: None,
            mutation_queue: None,
            tab_index,
        }
    }

    /// Open or create the full snapshot store and publish the current `doc`.
    pub fn ensure_full_snapshot(&mut self, doc: &CadDocument) -> Option<DocumentFullSnapshotInfo> {
        if self.full_snapshot.is_none() {
            let mut store = DocumentFullSnapshotStore::new(self.tab_index).ok()?;
            store.publish(doc).ok()?;
            self.full_snapshot = Some(store);
        }
        let store = self.full_snapshot.as_ref()?;
        Some(DocumentFullSnapshotInfo {
            path: store.path().to_string_lossy().to_string(),
            version: store.version(),
        })
    }

    /// Open or create the mutation queue.
    pub fn ensure_mutation_queue(&mut self) -> Option<DocumentMutationQueueInfo> {
        if self.mutation_queue.is_none() {
            self.mutation_queue = Some(DocumentMutationQueue::new(self.tab_index).ok()?);
        }
        let queue = self.mutation_queue.as_ref()?;
        Some(DocumentMutationQueueInfo {
            path: queue.path().to_string_lossy().to_string(),
        })
    }

    /// Publish `doc` to the full snapshot store.
    pub fn publish_full_snapshot(&mut self, doc: &CadDocument) {
        if let Some(ref mut store) = self.full_snapshot {
            if let Err(e) = store.publish(doc) {
                eprintln!(
                    "[DocumentShmResources] failed to publish full snapshot for tab {}: {e}",
                    self.tab_index
                );
            }
        }
    }

    /// Drain the mutation queue, calling `on_error` for each dropped op.
    pub fn drain_mutation_queue<F>(&mut self, mut on_error: F) -> Vec<EntityOp>
    where
        F: FnMut(&str),
    {
        match self.mutation_queue.as_mut() {
            Some(queue) => queue.drain(&mut on_error),
            None => Vec::new(),
        }
    }
}

#[cfg(test)]
mod full_tests {
    use super::*;
    use acadrust::entities::Point;
    use acadrust::tables::Layer;

    fn sample_doc() -> CadDocument {
        let mut doc = CadDocument::new();
        doc.layers.add(Layer::new("SURVEY")).unwrap();
        let mut point = Point::from_coords(10.0, 20.0, 5.0);
        point.common.layer = "SURVEY".to_string();
        doc.add_entity(EntityType::Point(point)).unwrap();
        doc
    }

    #[test]
    fn full_snapshot_roundtrip() {
        let doc = sample_doc();
        let mut store = DocumentFullSnapshotStore::new(0).unwrap();
        store.publish(&doc).unwrap();

        let mut reader = DocumentFullSnapshotReader::open(store.path()).unwrap();
        let (got, version) = reader.refresh().unwrap();
        assert_ne!(version, 0);
        assert_eq!(got.entities().count(), 1);
        assert_eq!(got.layers.get("SURVEY").unwrap().name, "SURVEY");
    }

    #[test]
    fn full_snapshot_updates_after_publish() {
        let doc = sample_doc();
        let mut store = DocumentFullSnapshotStore::new(0).unwrap();
        store.publish(&doc).unwrap();

        let mut reader = DocumentFullSnapshotReader::open(store.path()).unwrap();
        let (_, v1) = reader.refresh().unwrap();

        let mut doc2 = doc;
        let mut point2 = Point::from_coords(1.0, 2.0, 3.0);
        point2.common.layer = "SURVEY".to_string();
        doc2.add_entity(EntityType::Point(point2)).unwrap();
        store.publish(&doc2).unwrap();

        let (_, v2) = reader.refresh().unwrap();
        assert_ne!(v1, v2);
        assert_eq!(reader.refresh().unwrap().0.entities().count(), 2);
    }

    #[test]
    fn mutation_queue_roundtrip() {
        let mut queue = DocumentMutationQueue::new(0).unwrap();
        let mut view = DocumentMutationView::open(queue.path()).unwrap();

        let mut point = Point::from_coords(1.0, 2.0, 3.0);
        point.common.layer = "0".to_string();
        assert!(view.push(&EntityOp::Add(EntityType::Point(point.clone()))).unwrap());
        assert!(view.push(&EntityOp::Remove(Handle::new(42))).unwrap());

        let mut errors = Vec::new();
        let ops = queue.drain(|e| errors.push(e.to_string()));
        assert!(errors.is_empty(), "errors: {errors:?}");
        assert_eq!(ops.len(), 2);
        assert!(matches!(ops[0], EntityOp::Add(_)));
        assert!(matches!(ops[1], EntityOp::Remove(h) if h == Handle::new(42)));
    }

    #[test]
    fn mutation_queue_push_many() {
        let mut queue = DocumentMutationQueue::new(0).unwrap();
        let mut view = DocumentMutationView::open(queue.path()).unwrap();

        let ops: Vec<EntityOp> = (0..10)
            .map(|i| {
                let mut p = Point::from_coords(i as f64, 0.0, 0.0);
                p.common.layer = "0".to_string();
                EntityOp::Add(EntityType::Point(p))
            })
            .collect();
        assert_eq!(view.push_many(ops.into_iter()).unwrap(), 10);
        let drained = queue.drain(|_| {});
        assert_eq!(drained.len(), 10);
    }
}
