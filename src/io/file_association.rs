//! "Make Open CAD Studio the default for .dwg / .dxf" — the platform-specific
//! plumbing behind the one-time first-launch prompt (see `app::update`'s
//! `AssocPrompt*` handlers).
//!
//! Each OS guards default file associations differently, so there is no single
//! cross-platform call:
//!
//!   * Windows — the actual default is protected by a per-user UserChoice hash
//!     an app cannot forge. The supported path is to open the OS's own
//!     per-app default-programs dialog via
//!     `IApplicationAssociationRegistrationUI::LaunchAdvancedAssociationUI`,
//!     passing the RegisteredApplications name the MSI registered
//!     ("Open CAD Studio"). The user confirms there.
//!   * Linux — `xdg-mime default` writes the association into the user's
//!     `mimeapps.list`; no separate consent step. The .desktop file already
//!     declares the matching `MimeType=` entries.
//!   * macOS — LaunchServices' `LSSetDefaultRoleHandlerForContentType` binds
//!     the DWG/DXF UTIs (declared in the bundle's Info.plist) to this app's
//!     bundle id.
//!
//! The work runs on a dedicated thread because the Windows dialog is modal and
//! would otherwise block the iced executor; the result is delivered back
//! through a oneshot channel that the async wrapper awaits.

/// Reverse-DNS bundle / app id, shared by the macOS handler binding and the
/// Linux desktop-file name. Matches `CFBundleIdentifier` in packaging/Info.plist
/// and the installed `*.desktop` basename.
#[cfg(any(target_os = "macos", target_os = "linux"))]
const APP_ID: &str = "io.github.HakanSeven12.OpenCadStudio";

/// Silently register this app as *a* handler (not necessarily the default) for
/// .dwg / .dxf, so it appears in the OS "Open with" list. Unlike
/// [`set_default_app`], this changes no defaults and shows no UI — it just makes
/// the running binary discoverable as a handler, which the installers already
/// do for installed builds but the portable .exe / AppImage otherwise lack.
///
/// Idempotent and best-effort; safe to call on every launch. Runs synchronously
/// (registry / small file writes), so callers should invoke it off the UI
/// thread.
pub fn register_as_handler() -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        windows_impl::register_handler()
    }
    #[cfg(target_os = "linux")]
    {
        linux_impl::register_handler()
    }
    #[cfg(target_os = "macos")]
    {
        // .app bundles are registered with LaunchServices automatically when
        // first launched or moved (it scans Info.plist's CFBundleDocumentTypes),
        // so there is nothing to do at runtime.
        Ok(())
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
    {
        Ok(())
    }
}

/// Try to make this app the default handler for .dwg and .dxf. Returns a short
/// human-readable status string on success, or an error message on failure.
pub async fn set_default_app() -> Result<String, String> {
    let (tx, rx) = iced::futures::channel::oneshot::channel();
    std::thread::Builder::new()
        .name("set-default-app".into())
        .spawn(move || {
            let _ = tx.send(set_default_app_blocking());
        })
        .map_err(|e| format!("could not start the default-app helper: {e}"))?;
    rx.await
        .unwrap_or_else(|_| Err("the default-app helper was cancelled".to_string()))
}

fn set_default_app_blocking() -> Result<String, String> {
    #[cfg(target_os = "windows")]
    {
        windows_impl::set_default()
    }
    #[cfg(target_os = "linux")]
    {
        linux_impl::set_default()
    }
    #[cfg(target_os = "macos")]
    {
        macos_impl::set_default()
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
    {
        Err("Setting the default application isn't supported on this platform.".to_string())
    }
}

// ── Windows ────────────────────────────────────────────────────────────────

#[cfg(target_os = "windows")]
mod windows_impl {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::core::{GUID, HRESULT};
    use windows_sys::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
        COINIT_APARTMENTTHREADED,
    };

    // CLSID_ApplicationAssociationRegistrationUI
    // {1968106D-F3B5-44CF-890E-116FCB9ECEF1}
    const CLSID_APP_ASSOC_UI: GUID = GUID {
        data1: 0x1968106D,
        data2: 0xF3B5,
        data3: 0x44CF,
        data4: [0x89, 0x0E, 0x11, 0x6F, 0xCB, 0x9E, 0xCE, 0xF1],
    };
    // IID_IApplicationAssociationRegistrationUI
    // {1F76A169-F994-40AC-8FC8-0959E8874710}
    const IID_APP_ASSOC_UI: GUID = GUID {
        data1: 0x1F76A169,
        data2: 0xF994,
        data3: 0x40AC,
        data4: [0x8F, 0xC8, 0x09, 0x59, 0xE8, 0x87, 0x47, 0x10],
    };

    // Hand-rolled vtable for IApplicationAssociationRegistrationUI (IUnknown +
    // its single method). windows-sys ships raw COM, so we drive the interface
    // through the vtable directly rather than depend on generated wrappers.
    #[repr(C)]
    struct IAppAssocUiVtbl {
        query_interface:
            unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT,
        add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
        release: unsafe extern "system" fn(*mut c_void) -> u32,
        launch_advanced_association_ui:
            unsafe extern "system" fn(*mut c_void, *const u16) -> HRESULT,
    }
    #[repr(C)]
    struct IAppAssocUi {
        vtbl: *const IAppAssocUiVtbl,
    }

    // Must match the RegisteredApplications value name in packaging/windows/main.wxs.
    const APP_REGISTRY_NAME: &str = "Open CAD Studio";

    pub(super) fn set_default() -> Result<String, String> {
        unsafe {
            let co = CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32);
            // S_OK (0) / S_FALSE (1) mean we initialised COM and must balance it
            // with CoUninitialize. Any other value: another init already owns the
            // thread's apartment, so we leave it alone.
            let owns_com = co == 0 || co == 1;

            let result = (|| {
                let mut obj: *mut c_void = std::ptr::null_mut();
                let hr = CoCreateInstance(
                    &CLSID_APP_ASSOC_UI,
                    std::ptr::null_mut(),
                    CLSCTX_INPROC_SERVER,
                    &IID_APP_ASSOC_UI,
                    &mut obj,
                );
                if hr < 0 || obj.is_null() {
                    return Err(format!(
                        "could not open the Windows default-apps dialog (0x{:08X})",
                        hr as u32
                    ));
                }
                let this = obj as *mut IAppAssocUi;
                let name: Vec<u16> = std::ffi::OsStr::new(APP_REGISTRY_NAME)
                    .encode_wide()
                    .chain(Some(0))
                    .collect();
                let hr = ((*(*this).vtbl).launch_advanced_association_ui)(obj, name.as_ptr());
                ((*(*this).vtbl).release)(obj);
                if hr < 0 {
                    return Err(format!(
                        "the Windows default-apps dialog returned an error (0x{:08X})",
                        hr as u32
                    ));
                }
                Ok("Opened the Windows default-apps dialog — tick .dwg / .dxf there.".to_string())
            })();

            if owns_com {
                CoUninitialize();
            }
            result
        }
    }

    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
        KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ,
    };

    fn wide(s: &str) -> Vec<u16> {
        std::ffi::OsStr::new(s).encode_wide().chain(Some(0)).collect()
    }

    // Create `subkey` under HKCU and write `data` into `value` (None = the key's
    // default value), as a REG_SZ string.
    fn set_string(subkey: &str, value: Option<&str>, data: &str) -> Result<(), String> {
        let subkey_w = wide(subkey);
        let mut hkey: HKEY = std::ptr::null_mut();
        let rc = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                subkey_w.as_ptr(),
                0,
                std::ptr::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_WRITE,
                std::ptr::null(),
                &mut hkey,
                std::ptr::null_mut(),
            )
        };
        if rc != 0 {
            return Err(format!("RegCreateKeyExW failed ({rc}) for {subkey}"));
        }
        let name_w = value.map(wide);
        let name_ptr = name_w.as_ref().map_or(std::ptr::null(), |v| v.as_ptr());
        let data_w = wide(data);
        // cbData counts bytes including the trailing NUL that `wide` appended.
        let cb = (data_w.len() * std::mem::size_of::<u16>()) as u32;
        let rc = unsafe {
            RegSetValueExW(hkey, name_ptr, 0, REG_SZ, data_w.as_ptr() as *const u8, cb)
        };
        unsafe { RegCloseKey(hkey) };
        if rc != 0 {
            return Err(format!("RegSetValueExW failed ({rc}) for {subkey}"));
        }
        Ok(())
    }

    /// Create a per-user ProgID (`HKCU\Software\Classes\<progid>`) with an icon
    /// and an open command, mirroring one of the MSI's per-machine ProgIDs.
    fn register_progid(exe: &str, progid: &str, description: &str) -> Result<(), String> {
        let base = format!(r"Software\Classes\{progid}");
        set_string(&base, None, description)?;
        set_string(&format!(r"{base}\DefaultIcon"), None, &format!("\"{exe}\",0"))?;
        set_string(
            &format!(r"{base}\shell\open\command"),
            None,
            &format!("\"{exe}\" \"%1\""),
        )?;
        Ok(())
    }

    /// Register the running .exe (per-user, HKCU) so the portable build matches
    /// what the MSI provides machine-wide:
    ///   * an `Applications\OpenCADStudio.exe` entry → listed under "Open with";
    ///   * ProgIDs + a Capabilities / RegisteredApplications block → the app is
    ///     a default-app candidate, so the in-app "set as default" prompt's OS
    ///     dialog (LaunchAdvancedAssociationUI "Open CAD Studio") finds it.
    pub(super) fn register_handler() -> Result<(), String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let exe = exe.to_string_lossy().to_string();

        // ── "Open with" exe listing ─────────────────────────────────────────
        const APP_BASE: &str = r"Software\Classes\Applications\OpenCADStudio.exe";
        set_string(
            &format!(r"{APP_BASE}\shell\open\command"),
            None,
            &format!("\"{exe}\" \"%1\""),
        )?;
        set_string(APP_BASE, Some("FriendlyAppName"), "Open CAD Studio")?;
        // DefaultIcon is what Windows uses to show the app icon in the
        // "Open with" context-menu list and the "Choose another app" picker.
        set_string(&format!(r"{APP_BASE}\DefaultIcon"), None, &format!("\"{exe}\",0"))?;
        // Listing the extensions under SupportedTypes is what makes the app
        // appear in the "Open with → Choose another app" picker for them.
        set_string(&format!(r"{APP_BASE}\SupportedTypes"), Some(".dwg"), "")?;
        set_string(&format!(r"{APP_BASE}\SupportedTypes"), Some(".dxf"), "")?;

        // ── ProgIDs (per-user mirror of the MSI's) ──────────────────────────
        // The Capabilities entries below point at these, and they must resolve
        // to a real open command for "set as default" to apply them.
        register_progid(&exe, "OpenCADStudio.DWG", "DWG Drawing")?;
        register_progid(&exe, "OpenCADStudio.DXF", "DXF Drawing")?;
        // Also offer the ProgIDs in each extension's Open-with list.
        set_string(
            r"Software\Classes\.dwg\OpenWithProgids",
            Some("OpenCADStudio.DWG"),
            "",
        )?;
        set_string(
            r"Software\Classes\.dxf\OpenWithProgids",
            Some("OpenCADStudio.DXF"),
            "",
        )?;

        // ── Capabilities + RegisteredApplications ───────────────────────────
        // Mirrors the MSI's DefaultPrograms component, but per-user, so the
        // portable build is a Default-Apps candidate too. The value name in
        // RegisteredApplications must equal the name passed to
        // LaunchAdvancedAssociationUI ("Open CAD Studio").
        const CAP: &str = r"Software\Open CAD Studio\Capabilities";
        set_string(CAP, Some("ApplicationName"), "Open CAD Studio")?;
        set_string(
            CAP,
            Some("ApplicationDescription"),
            "2D/3D CAD application for DWG and DXF drawings.",
        )?;
        set_string(&format!(r"{CAP}\FileAssociations"), Some(".dwg"), "OpenCADStudio.DWG")?;
        set_string(&format!(r"{CAP}\FileAssociations"), Some(".dxf"), "OpenCADStudio.DXF")?;
        set_string(
            r"Software\RegisteredApplications",
            Some("Open CAD Studio"),
            r"Software\Open CAD Studio\Capabilities",
        )?;
        Ok(())
    }
}

// ── Linux ──────────────────────────────────────────────────────────────────

#[cfg(target_os = "linux")]
mod linux_impl {
    use super::APP_ID;
    use std::path::{Path, PathBuf};

    /// Write a user-level .desktop file pointing at the running binary, then
    /// refresh the MIME cache so file managers list us under "Open with".
    /// Skipped when a system package already registered the app, so we don't
    /// shadow a distro-provided desktop entry.
    pub(super) fn register_handler() -> Result<(), String> {
        // A system install already lists us — leave it alone.
        let data_dirs = std::env::var("XDG_DATA_DIRS")
            .unwrap_or_else(|_| "/usr/local/share:/usr/share".to_string());
        let already_system = data_dirs.split(':').filter(|d| !d.is_empty()).any(|d| {
            Path::new(d)
                .join("applications")
                .join(format!("{APP_ID}.desktop"))
                .exists()
        });
        if already_system {
            return Ok(());
        }

        // Prefer the AppImage path ($APPIMAGE) — current_exe() points inside the
        // transient mount and isn't a stable command to launch later.
        let exec = std::env::var_os("APPIMAGE")
            .map(PathBuf::from)
            .or_else(|| std::env::current_exe().ok())
            .ok_or("could not determine the executable path")?;
        let exec = exec.to_string_lossy();

        let apps = data_home()?.join("applications");
        let desktop_path = apps.join(format!("{APP_ID}.desktop"));

        let contents = format!(
            "[Desktop Entry]\n\
             Name=Open CAD Studio\n\
             Comment=A CAD application for 2D/3D drawing and design\n\
             Exec={exec} %f\n\
             Icon={APP_ID}\n\
             Terminal=false\n\
             Type=Application\n\
             Categories=Graphics;Engineering;\n\
             MimeType=image/vnd.dwg;image/vnd.dxf;\n\
             Keywords=CAD;DWG;DXF;drawing;design;\n\
             StartupWMClass=OpenCADStudio\n"
        );

        // Nothing changed since last launch → skip the write and the (slow)
        // desktop-database refresh.
        if std::fs::read_to_string(&desktop_path).ok().as_deref() == Some(contents.as_str()) {
            return Ok(());
        }

        std::fs::create_dir_all(&apps).map_err(|e| e.to_string())?;
        std::fs::write(&desktop_path, &contents).map_err(|e| e.to_string())?;

        install_icon()?;

        // Best-effort: refresh the MIME→app cache. Absent on minimal systems.
        let _ = std::process::Command::new("update-desktop-database")
            .arg(&apps)
            .status();
        Ok(())
    }

    /// Write the app SVG icon into the user's hicolor icon theme so that the
    /// named `Icon={APP_ID}` entry in the .desktop file resolves correctly in
    /// file managers and the "Open with" picker. Skipped when the icon is
    /// already up-to-date; best-effort icon-cache refresh afterwards.
    fn install_icon() -> Result<(), String> {
        static LOGO_SVG: &[u8] = include_bytes!("../../assets/logo.svg");

        let icon_dir = data_home()?.join("icons/hicolor/scalable/apps");
        let icon_path = icon_dir.join(format!("{APP_ID}.svg"));

        // Skip write when the on-disk file is already identical.
        if std::fs::read(&icon_path).ok().as_deref() == Some(LOGO_SVG) {
            return Ok(());
        }

        std::fs::create_dir_all(&icon_dir).map_err(|e| e.to_string())?;
        std::fs::write(&icon_path, LOGO_SVG).map_err(|e| e.to_string())?;

        // Refresh the icon cache so desktop environments pick up the new file.
        let hicolor = data_home()
            .map(|d| d.join("icons/hicolor"))
            .unwrap_or_default();
        let _ = std::process::Command::new("gtk-update-icon-cache")
            .args(["--force", "--quiet", &hicolor.to_string_lossy()])
            .status();
        Ok(())
    }

    fn data_home() -> Result<PathBuf, String> {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
            .ok_or_else(|| "neither XDG_DATA_HOME nor HOME is set".to_string())
    }

    pub(super) fn set_default() -> Result<String, String> {
        let desktop = format!("{APP_ID}.desktop");
        let status = std::process::Command::new("xdg-mime")
            .args(["default", &desktop, "image/vnd.dwg", "image/vnd.dxf"])
            .status()
            .map_err(|e| format!("could not run xdg-mime: {e}"))?;
        if status.success() {
            Ok("Open CAD Studio is now the default for .dwg and .dxf files.".to_string())
        } else {
            Err(format!("xdg-mime exited with {status}"))
        }
    }
}

// ── macOS ──────────────────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
mod macos_impl {
    use super::APP_ID;
    use std::ffi::c_void;

    #[repr(C)]
    struct __CFString {
        _private: [u8; 0],
    }
    type CFStringRef = *const __CFString;
    type CFAllocatorRef = *const c_void;
    type OSStatus = i32;
    type LSRolesMask = u32;

    const KCFSTRING_ENCODING_UTF8: u32 = 0x0800_0100;
    const KLS_ROLES_ALL: LSRolesMask = 0xFFFF_FFFF;

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        static kCFAllocatorDefault: CFAllocatorRef;
        fn CFStringCreateWithBytes(
            alloc: CFAllocatorRef,
            bytes: *const u8,
            num_bytes: isize,
            encoding: u32,
            is_external_representation: u8,
        ) -> CFStringRef;
        fn CFRelease(cf: *const c_void);
    }

    // LaunchServices lives under the CoreServices umbrella framework.
    #[link(name = "CoreServices", kind = "framework")]
    extern "C" {
        fn LSSetDefaultRoleHandlerForContentType(
            in_content_type: CFStringRef,
            in_role: LSRolesMask,
            in_handler_bundle_id: CFStringRef,
        ) -> OSStatus;
    }

    fn cfstr(s: &str) -> CFStringRef {
        unsafe {
            CFStringCreateWithBytes(
                kCFAllocatorDefault,
                s.as_ptr(),
                s.len() as isize,
                KCFSTRING_ENCODING_UTF8,
                0,
            )
        }
    }

    pub(super) fn set_default() -> Result<String, String> {
        let bundle = cfstr(APP_ID);
        if bundle.is_null() {
            return Err("could not build the bundle-id string".to_string());
        }
        // UTIs declared in packaging/Info.plist's CFBundleDocumentTypes.
        let mut last_err: Option<String> = None;
        for uti in ["com.autodesk.dwg", "com.autodesk.dxf"] {
            let ct = cfstr(uti);
            if ct.is_null() {
                last_err = Some("could not build the content-type string".to_string());
                continue;
            }
            let st = unsafe { LSSetDefaultRoleHandlerForContentType(ct, KLS_ROLES_ALL, bundle) };
            unsafe { CFRelease(ct as *const c_void) };
            if st != 0 {
                last_err = Some(format!("LaunchServices error {st} for {uti}"));
            }
        }
        unsafe { CFRelease(bundle as *const c_void) };
        match last_err {
            None => Ok("Open CAD Studio is now the default for .dwg and .dxf files.".to_string()),
            Some(e) => Err(e),
        }
    }
}
