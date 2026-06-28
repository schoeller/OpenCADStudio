use acadrust::entities::{Insert, MText};
use acadrust::tables::{BlockRecord, TextStyle};
use acadrust::types::Vector3;
use acadrust::{CadDocument, EntityType, Handle};
use OpenCADStudio::scene::cache::block_cache::{expand_insert, BlockCache};

fn drawable_point_count(wires: &[OpenCADStudio::scene::WireModel]) -> usize {
    wires
        .iter()
        .map(|w| w.points.iter().filter(|p| p[0].is_finite()).count() + w.fill_tris.len())
        .sum()
}

#[test]
fn block_nested_mtext_uses_its_style_font() {
    let mut doc = CadDocument::new();

    let mut style = TextStyle::new("SHOP");
    style.font_file = "arial.ttf".to_string();
    doc.text_styles.add(style).unwrap();

    let br_h = Handle::new(doc.next_handle());
    let mut br = BlockRecord::new("LABEL_BLOCK");
    br.handle = br_h;
    doc.block_records.add(br).unwrap();

    let mut mtext = MText::with_value("FERRAGAMO", Vector3::new(0.0, 0.0, 0.0));
    mtext.style = "SHOP".to_string();
    mtext.height = 20.0;
    mtext.rectangle_width = 0.0;
    let mut sub = EntityType::MText(mtext);
    sub.common_mut().owner_handle = br_h;
    doc.add_entity(sub).unwrap();

    let ins = Insert::new("LABEL_BLOCK", Vector3::new(100.0, 50.0, 0.0));
    doc.add_entity(EntityType::Insert(ins.clone())).unwrap();
    let cache = BlockCache::build(&doc, 1.0, [0.0, 0.0, 0.0, 1.0]);
    let wires = expand_insert(
        &cache,
        &ins,
        Handle::new(999),
        [1.0, 1.0, 1.0, 1.0],
        0.0,
        [0.0; 8],
        1.0,
        false,
        1.0,
        None,
        None,
        false,
        [0.0, 0.0, 0.0, 1.0],
    )
    .expect("block defn is cached");

    assert!(
        drawable_point_count(&wires) > 0,
        "block-nested MTEXT should render through its text style font"
    );

    assert!(
        wires.iter().all(|w| w.points.is_empty() || w.fill_tris.is_empty()),
        "outline and fill wires should be separate for correct GPU classification"
    );
}
