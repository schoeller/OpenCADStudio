//! Human-readable and DXF-canonical names for each `EntityType` variant.
//!
//! Two functions because they're used for different audiences:
//!   - `ui_name`  — properties panel / status bar (mixed-case, friendly).
//!   - `dxf_name` — command-line output / file format (uppercase, matches
//!                  the DXF entity record type name).
//!
//! Co-located so adding a new variant only requires updating one file.

use acadrust::EntityType;

/// Mixed-case display name shown to the user in panels, tooltips, etc.
pub fn ui_name(e: &EntityType) -> &'static str {
    match e {
        EntityType::Point(_) => "Point",
        EntityType::Line(_) => "Line",
        EntityType::Circle(_) => "Circle",
        EntityType::Arc(_) => "Arc",
        EntityType::Ellipse(_) => "Ellipse",
        EntityType::Spline(_) => "Spline",
        EntityType::Helix(_) => "Helix",
        EntityType::LwPolyline(_) => "Polyline",
        EntityType::Polyline(_) => "Polyline",
        EntityType::Polyline2D(_) => "Polyline2D",
        EntityType::Polyline3D(_) => "Polyline3D",
        EntityType::PolyfaceMesh(_) => "PolyfaceMesh",
        EntityType::PolygonMesh(_) => "PolygonMesh",
        EntityType::Text(_) => "Text",
        EntityType::MText(_) => "MText",
        EntityType::Dimension(_) => "Dimension",
        EntityType::Leader(_) => "Leader",
        EntityType::MultiLeader(_) => "MultiLeader",
        EntityType::Tolerance(_) => "Tolerance",
        EntityType::Insert(_) => "Block Reference",
        EntityType::Block(_) => "Block",
        EntityType::BlockEnd(_) => "Block End",
        EntityType::Hatch(_) => "Hatch",
        EntityType::Solid(_) => "Solid",
        EntityType::Face3D(_) => "3D Face",
        EntityType::Solid3D(_) => "3D Solid",
        EntityType::Region(_) => "Region",
        EntityType::Body(_) => "Body",
        EntityType::Surface(_) => "Surface",
        EntityType::Mesh(_) => "Mesh",
        EntityType::Ray(_) => "Ray",
        EntityType::XLine(_) => "XLine",
        EntityType::MLine(_) => "MLine",
        EntityType::Viewport(_) => "Viewport",
        EntityType::RasterImage(_) => "Raster Image",
        EntityType::Wipeout(_) => "Wipeout",
        EntityType::Underlay(_) => "Underlay",
        EntityType::Shape(_) => "Shape",
        EntityType::Table(_) => "Table",
        EntityType::AttributeDefinition(_) => "Attribute Definition",
        EntityType::AttributeEntity(_) => "Attribute",
        EntityType::Ole2Frame(_) => "OLE Frame",
        EntityType::Light(_) => "Light",
        EntityType::Seqend(_) => "Seqend",
        EntityType::Unknown(_) => "Unknown",
    }
}

/// Canonical DXF entity type record name (uppercase), as used by AutoCAD's
/// command-line LIST output and the DXF file format. Variants without a
/// well-defined DXF name fall back to "ENTITY".
pub fn dxf_name(e: &EntityType) -> &'static str {
    match e {
        EntityType::Line(_) => "LINE",
        EntityType::Circle(_) => "CIRCLE",
        EntityType::Arc(_) => "ARC",
        EntityType::LwPolyline(_) => "LWPOLYLINE",
        EntityType::Polyline(_) => "POLYLINE",
        EntityType::Polyline2D(_) => "POLYLINE2D",
        EntityType::Polyline3D(_) => "POLYLINE3D",
        EntityType::Text(_) => "TEXT",
        EntityType::MText(_) => "MTEXT",
        EntityType::Insert(_) => "INSERT",
        EntityType::Hatch(_) => "HATCH",
        EntityType::Dimension(_) => "DIMENSION",
        EntityType::Viewport(_) => "VIEWPORT",
        EntityType::Spline(_) => "SPLINE",
        EntityType::Helix(_) => "HELIX",
        EntityType::Ellipse(_) => "ELLIPSE",
        EntityType::Point(_) => "POINT",
        EntityType::Ray(_) => "RAY",
        EntityType::XLine(_) => "XLINE",
        EntityType::Face3D(_) => "3DFACE",
        EntityType::Table(_) => "TABLE",
        EntityType::MLine(_) => "MLINE",
        EntityType::RasterImage(_) => "RASTERIMAGE",
        EntityType::Wipeout(_) => "WIPEOUT",
        EntityType::Underlay(_) => "UNDERLAY",
        EntityType::AttributeDefinition(_) => "ATTDEF",
        EntityType::AttributeEntity(_) => "ATTRIB",
        EntityType::Leader(_) => "LEADER",
        EntityType::MultiLeader(_) => "MULTILEADER",
        EntityType::Tolerance(_) => "TOLERANCE",
        EntityType::Shape(_) => "SHAPE",
        EntityType::Solid(_) => "SOLID",
        EntityType::Solid3D(_) => "3DSOLID",
        EntityType::Region(_) => "REGION",
        EntityType::Body(_) => "BODY",
        EntityType::Surface(_) => "SURFACE",
        EntityType::Mesh(_) => "MESH",
        EntityType::Light(_) => "LIGHT",
        _ => "ENTITY",
    }
}
