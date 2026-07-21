//! Pythonic entity wrappers and constructors.

use pyo3::prelude::*;

use acadrust::{EntityType, Handle};
use ocs_plugin_api::shm::EntityOp;

use crate::geometry::PyVector3;

/// Build a batch of `EntityOp::Add(Line)` operations from raw endpoint pairs.
pub(crate) fn line_ops(
    lines: Vec<((f64, f64, f64), (f64, f64, f64))>,
    layer: String,
) -> Vec<EntityOp> {
    use acadrust::entities::Line;
    use acadrust::types::Vector3;
    lines
        .into_iter()
        .map(|((x1, y1, z1), (x2, y2, z2))| {
            let mut l = Line::from_points(
                Vector3::new(x1, y1, z1),
                Vector3::new(x2, y2, z2),
            );
            l.common.layer = layer.clone();
            EntityOp::Add(EntityType::Line(l))
        })
        .collect()
}

/// Build a batch of `EntityOp::Add(Point)` operations from raw coordinates.
/// Used by the fast vectorized `Document.add_points` path to avoid creating
/// one Python `Point` object per entity.
pub(crate) fn point_ops(points: Vec<(f64, f64, f64)>, layer: String) -> Vec<EntityOp> {
    use acadrust::entities::Point;
    use acadrust::types::Vector3;
    points
        .into_iter()
        .map(|(x, y, z)| {
            let mut p = Point::at(Vector3::new(x, y, z));
            p.common.layer = layer.clone();
            EntityOp::Add(EntityType::Point(p))
        })
        .collect()
}

/// Return a short type name for an enum variant (e.g. "Point", "Hatch").
fn entity_kind_name(entity: impl std::fmt::Debug) -> String {
    let s = format!("{:?}", entity);
    s.split('(').next().unwrap_or("Entity").to_string()
}

/// Queue a remove operation for the given handle. Shared by `Entity.delete` and
/// all concrete entity subclasses.
fn queue_remove_handle(handle: u64) -> PyResult<()> {
    let mut queue = crate::document::open_queue()?;
    if queue
        .push(&EntityOp::Remove(Handle::new(handle)))
        .map_err(crate::document::queue_err)?
    {
        Ok(())
    } else {
        Err(pyo3::exceptions::PyRuntimeError::new_err(
            "mutation queue full; call commit() more often",
        ))
    }
}

#[pyclass(name = "Entity")]
#[derive(Clone)]
pub struct PyEntity {
    pub handle: u64,
    pub kind: String,
    pub layer: String,
}

#[pymethods]
impl PyEntity {
    #[getter]
    fn handle(&self) -> u64 {
        self.handle
    }

    #[getter]
    fn kind(&self) -> String {
        self.kind.clone()
    }

    #[getter]
    fn layer(&self) -> String {
        self.layer.clone()
    }

    fn delete(&self) -> PyResult<()> {
        queue_remove_handle(self.handle)
    }
}

#[pyclass(name = "Point")]
#[derive(Clone)]
pub struct PyPoint {
    pub location: PyVector3,
    pub layer: String,
}

#[pymethods]
impl PyPoint {
    #[new]
    #[pyo3(signature = (x=0.0, y=0.0, z=0.0, layer=None))]
    fn new(x: f64, y: f64, z: f64, layer: Option<String>) -> Self {
        Self {
            location: PyVector3 { x, y, z },
            layer: layer.unwrap_or_else(|| "0".to_string()),
        }
    }

    #[getter]
    fn x(&self) -> f64 {
        self.location.x
    }

    #[getter]
    fn y(&self) -> f64 {
        self.location.y
    }

    #[getter]
    fn z(&self) -> f64 {
        self.location.z
    }

    #[getter]
    fn point(&self) -> PyVector3 {
        self.location.clone()
    }

    #[getter]
    fn layer(&self) -> String {
        self.layer.clone()
    }
}

#[pyclass(name = "Line")]
#[derive(Clone)]
pub struct PyLine {
    pub start: PyVector3,
    pub end: PyVector3,
    pub layer: String,
}

#[pymethods]
impl PyLine {
    #[new]
    #[pyo3(signature = (start=None, end=None, layer=None))]
    fn new(
        start: Option<PyVector3>,
        end: Option<PyVector3>,
        layer: Option<String>,
    ) -> Self {
        Self {
            start: start.unwrap_or(PyVector3 { x: 0.0, y: 0.0, z: 0.0 }),
            end: end.unwrap_or(PyVector3 { x: 1.0, y: 1.0, z: 0.0 }),
            layer: layer.unwrap_or_else(|| "0".to_string()),
        }
    }

    #[getter]
    fn start(&self) -> PyVector3 {
        self.start.clone()
    }

    #[getter]
    fn end(&self) -> PyVector3 {
        self.end.clone()
    }

    #[getter]
    fn layer(&self) -> String {
        self.layer.clone()
    }
}

#[pyclass(name = "Circle")]
#[derive(Clone)]
pub struct PyCircle {
    pub center: PyVector3,
    pub radius: f64,
    pub layer: String,
}

#[pymethods]
impl PyCircle {
    #[new]
    #[pyo3(signature = (center=None, radius=1.0, layer=None))]
    fn new(center: Option<PyVector3>, radius: f64, layer: Option<String>) -> Self {
        Self {
            center: center.unwrap_or(PyVector3 { x: 0.0, y: 0.0, z: 0.0 }),
            radius,
            layer: layer.unwrap_or_else(|| "0".to_string()),
        }
    }

    #[getter]
    fn center(&self) -> PyVector3 {
        self.center.clone()
    }

    #[getter]
    fn radius(&self) -> f64 {
        self.radius
    }

    #[getter]
    fn layer(&self) -> String {
        self.layer.clone()
    }
}

#[pyclass(name = "Arc")]
#[derive(Clone)]
pub struct PyArc {
    pub center: PyVector3,
    pub radius: f64,
    pub start_angle: f64,
    pub end_angle: f64,
    pub layer: String,
}

#[pymethods]
impl PyArc {
    #[new]
    #[pyo3(signature = (center=None, radius=1.0, start_angle=0.0, end_angle=1.57079632679, layer=None))]
    fn new(
        center: Option<PyVector3>,
        radius: f64,
        start_angle: f64,
        end_angle: f64,
        layer: Option<String>,
    ) -> Self {
        Self {
            center: center.unwrap_or(PyVector3 { x: 0.0, y: 0.0, z: 0.0 }),
            radius,
            start_angle,
            end_angle,
            layer: layer.unwrap_or_else(|| "0".to_string()),
        }
    }

    #[getter]
    fn center(&self) -> PyVector3 {
        self.center.clone()
    }

    #[getter]
    fn radius(&self) -> f64 {
        self.radius
    }

    #[getter]
    fn start_angle(&self) -> f64 {
        self.start_angle
    }

    #[getter]
    fn end_angle(&self) -> f64 {
        self.end_angle
    }

    #[getter]
    fn layer(&self) -> String {
        self.layer.clone()
    }
}

#[pyclass(name = "Text")]
#[derive(Clone)]
pub struct PyText {
    pub value: String,
    pub location: PyVector3,
    pub height: f64,
    pub layer: String,
}

#[pymethods]
impl PyText {
    #[new]
    #[pyo3(signature = (value=None, x=0.0, y=0.0, z=0.0, height=2.5, layer=None))]
    fn new(value: Option<String>, x: f64, y: f64, z: f64, height: f64, layer: Option<String>) -> Self {
        Self {
            value: value.unwrap_or_default(),
            location: PyVector3 { x, y, z },
            height,
            layer: layer.unwrap_or_else(|| "0".to_string()),
        }
    }

    #[getter]
    fn value(&self) -> String {
        self.value.clone()
    }

    #[getter]
    fn location(&self) -> PyVector3 {
        self.location.clone()
    }

    #[getter]
    fn height(&self) -> f64 {
        self.height
    }

    #[getter]
    fn layer(&self) -> String {
        self.layer.clone()
    }
}

#[pyclass(name = "MText")]
#[derive(Clone)]
pub struct PyMText {
    pub value: String,
    pub insertion_point: PyVector3,
    pub height: f64,
    pub layer: String,
}

#[pymethods]
impl PyMText {
    #[new]
    #[pyo3(signature = (value=None, x=0.0, y=0.0, z=0.0, height=2.5, layer=None))]
    fn new(
        value: Option<String>,
        x: f64,
        y: f64,
        z: f64,
        height: f64,
        layer: Option<String>,
    ) -> Self {
        Self {
            value: value.unwrap_or_default(),
            insertion_point: PyVector3 { x, y, z },
            height,
            layer: layer.unwrap_or_else(|| "0".to_string()),
        }
    }

    #[getter]
    fn value(&self) -> String {
        self.value.clone()
    }

    #[getter]
    fn insertion_point(&self) -> PyVector3 {
        self.insertion_point.clone()
    }

    #[getter]
    fn height(&self) -> f64 {
        self.height
    }

    #[getter]
    fn layer(&self) -> String {
        self.layer.clone()
    }
}

#[pyclass(name = "LwPolyline")]
#[derive(Clone)]
pub struct PyLwPolyline {
    pub vertices: Vec<(f64, f64, f64)>, // x, y, bulge
    pub is_closed: bool,
    pub layer: String,
}

#[pymethods]
impl PyLwPolyline {
    #[new]
    #[pyo3(signature = (vertices=None, is_closed=false, layer=None))]
    fn new(
        vertices: Option<Vec<(f64, f64, f64)>>,
        is_closed: bool,
        layer: Option<String>,
    ) -> Self {
        Self {
            vertices: vertices.unwrap_or_default(),
            is_closed,
            layer: layer.unwrap_or_else(|| "0".to_string()),
        }
    }

    #[getter]
    fn vertices(&self) -> Vec<(f64, f64, f64)> {
        self.vertices.clone()
    }

    #[getter]
    fn is_closed(&self) -> bool {
        self.is_closed
    }

    #[getter]
    fn layer(&self) -> String {
        self.layer.clone()
    }
}

#[pyclass(name = "Insert")]
#[derive(Clone)]
pub struct PyInsert {
    pub block_name: String,
    pub insertion_point: PyVector3,
    pub rotation: f64,
    pub layer: String,
}

#[pymethods]
impl PyInsert {
    #[new]
    #[pyo3(signature = (block_name, x=0.0, y=0.0, z=0.0, rotation=0.0, layer=None))]
    fn new(
        block_name: String,
        x: f64,
        y: f64,
        z: f64,
        rotation: f64,
        layer: Option<String>,
    ) -> Self {
        Self {
            block_name,
            insertion_point: PyVector3 { x, y, z },
            rotation,
            layer: layer.unwrap_or_else(|| "0".to_string()),
        }
    }

    #[getter]
    fn block_name(&self) -> String {
        self.block_name.clone()
    }

    #[getter]
    fn insertion_point(&self) -> PyVector3 {
        self.insertion_point.clone()
    }

    #[getter]
    fn rotation(&self) -> f64 {
        self.rotation
    }

    #[getter]
    fn layer(&self) -> String {
        self.layer.clone()
    }
}

#[pyclass(name = "Hatch")]
#[derive(Clone)]
pub struct PyHatch {
    pub boundary: Vec<(f64, f64)>,
    pub is_solid: bool,
    pub layer: String,
}

#[pymethods]
impl PyHatch {
    #[new]
    #[pyo3(signature = (boundary=None, is_solid=true, layer=None))]
    fn new(
        boundary: Option<Vec<(f64, f64)>>,
        is_solid: bool,
        layer: Option<String>,
    ) -> Self {
        Self {
            boundary: boundary.unwrap_or_default(),
            is_solid,
            layer: layer.unwrap_or_else(|| "0".to_string()),
        }
    }

    #[getter]
    fn boundary(&self) -> Vec<(f64, f64)> {
        self.boundary.clone()
    }

    #[getter]
    fn is_solid(&self) -> bool {
        self.is_solid
    }

    #[getter]
    fn layer(&self) -> String {
        self.layer.clone()
    }
}

#[pyclass(name = "Dimension")]
#[derive(Clone)]
pub struct PyDimension {
    pub start: PyVector3,
    pub end: PyVector3,
    pub layer: String,
}

#[pymethods]
impl PyDimension {
    #[new]
    #[pyo3(signature = (start=None, end=None, layer=None))]
    fn new(
        start: Option<PyVector3>,
        end: Option<PyVector3>,
        layer: Option<String>,
    ) -> Self {
        Self {
            start: start.unwrap_or(PyVector3 { x: 0.0, y: 0.0, z: 0.0 }),
            end: end.unwrap_or(PyVector3 { x: 10.0, y: 0.0, z: 0.0 }),
            layer: layer.unwrap_or_else(|| "0".to_string()),
        }
    }

    #[getter]
    fn start(&self) -> PyVector3 {
        self.start.clone()
    }

    #[getter]
    fn end(&self) -> PyVector3 {
        self.end.clone()
    }

    #[getter]
    fn layer(&self) -> String {
        self.layer.clone()
    }
}

#[pyclass(name = "Leader")]
#[derive(Clone)]
pub struct PyLeader {
    pub vertices: Vec<PyVector3>,
    pub layer: String,
}

#[pymethods]
impl PyLeader {
    #[new]
    #[pyo3(signature = (vertices=None, layer=None))]
    fn new(
        vertices: Option<Vec<PyVector3>>,
        layer: Option<String>,
    ) -> Self {
        Self {
            vertices: vertices.unwrap_or_default(),
            layer: layer.unwrap_or_else(|| "0".to_string()),
        }
    }

    #[getter]
    fn vertices(&self) -> Vec<PyVector3> {
        self.vertices.clone()
    }

    #[getter]
    fn layer(&self) -> String {
        self.layer.clone()
    }
}

#[pyclass(name = "Viewport")]
#[derive(Clone)]
pub struct PyViewport {
    pub center: PyVector3,
    pub width: f64,
    pub height: f64,
    pub id: i16,
    pub layer: String,
}

#[pymethods]
impl PyViewport {
    #[new]
    #[pyo3(signature = (center=None, width=1.0, height=1.0, id=2, layer=None))]
    fn new(
        center: Option<PyVector3>,
        width: f64,
        height: f64,
        id: i16,
        layer: Option<String>,
    ) -> Self {
        Self {
            center: center.unwrap_or(PyVector3 { x: 0.0, y: 0.0, z: 0.0 }),
            width,
            height,
            id,
            layer: layer.unwrap_or_else(|| "0".to_string()),
        }
    }

    #[getter]
    fn center(&self) -> PyVector3 {
        self.center.clone()
    }

    #[getter]
    fn width(&self) -> f64 {
        self.width
    }

    #[getter]
    fn height(&self) -> f64 {
        self.height
    }

    #[getter]
    fn id(&self) -> i16 {
        self.id
    }

    #[getter]
    fn layer(&self) -> String {
        self.layer.clone()
    }
}

#[pyclass(name = "Spline")]
#[derive(Clone)]
pub struct PySpline {
    pub control_points: Vec<PyVector3>,
    pub knots: Vec<f64>,
    pub degree: i16,
    pub is_closed: bool,
    pub layer: String,
}

#[pymethods]
impl PySpline {
    #[new]
    #[pyo3(signature = (control_points=None, knots=None, degree=3, is_closed=false, layer=None))]
    fn new(
        control_points: Option<Vec<PyVector3>>,
        knots: Option<Vec<f64>>,
        degree: i16,
        is_closed: bool,
        layer: Option<String>,
    ) -> Self {
        Self {
            control_points: control_points.unwrap_or_default(),
            knots: knots.unwrap_or_default(),
            degree,
            is_closed,
            layer: layer.unwrap_or_else(|| "0".to_string()),
        }
    }

    #[getter]
    fn control_points(&self) -> Vec<PyVector3> {
        self.control_points.clone()
    }

    #[getter]
    fn knots(&self) -> Vec<f64> {
        self.knots.clone()
    }

    #[getter]
    fn degree(&self) -> i16 {
        self.degree
    }

    #[getter]
    fn is_closed(&self) -> bool {
        self.is_closed
    }

    #[getter]
    fn layer(&self) -> String {
        self.layer.clone()
    }
}

#[pyfunction]
#[pyo3(signature = (boundary=None, is_solid=true, layer=None))]
pub fn make_hatch(
    boundary: Option<Vec<(f64, f64)>>,
    is_solid: bool,
    layer: Option<String>,
) -> PyHatch {
    PyHatch::new(boundary, is_solid, layer)
}

#[pyfunction]
#[pyo3(signature = (start=None, end=None, layer=None))]
pub fn make_dimension(
    start: Option<PyVector3>,
    end: Option<PyVector3>,
    layer: Option<String>,
) -> PyDimension {
    PyDimension::new(start, end, layer)
}

#[pyfunction]
#[pyo3(signature = (vertices=None, layer=None))]
pub fn make_leader(
    vertices: Option<Vec<PyVector3>>,
    layer: Option<String>,
) -> PyLeader {
    PyLeader::new(vertices, layer)
}

#[pyfunction]
#[pyo3(signature = (center=None, width=1.0, height=1.0, id=2, layer=None))]
pub fn make_viewport(
    center: Option<PyVector3>,
    width: f64,
    height: f64,
    id: i16,
    layer: Option<String>,
) -> PyViewport {
    PyViewport::new(center, width, height, id, layer)
}

#[pyfunction]
#[pyo3(signature = (control_points=None, knots=None, degree=3, is_closed=false, layer=None))]
pub fn make_spline(
    control_points: Option<Vec<PyVector3>>,
    knots: Option<Vec<f64>>,
    degree: i16,
    is_closed: bool,
    layer: Option<String>,
) -> PySpline {
    PySpline::new(control_points, knots, degree, is_closed, layer)
}

#[pyfunction]
#[pyo3(signature = (value=None, x=0.0, y=0.0, z=0.0, height=2.5, layer=None))]
pub fn make_mtext(
    value: Option<String>,
    x: f64,
    y: f64,
    z: f64,
    height: f64,
    layer: Option<String>,
) -> PyMText {
    PyMText::new(value, x, y, z, height, layer)
}

#[pyfunction]
#[pyo3(signature = (vertices=None, is_closed=false, layer=None))]
pub fn make_lwpolyline(
    vertices: Option<Vec<(f64, f64, f64)>>,
    is_closed: bool,
    layer: Option<String>,
) -> PyLwPolyline {
    PyLwPolyline::new(vertices, is_closed, layer)
}

#[pyfunction]
#[pyo3(signature = (block_name, x=0.0, y=0.0, z=0.0, rotation=0.0, layer=None))]
pub fn make_insert(
    block_name: String,
    x: f64,
    y: f64,
    z: f64,
    rotation: f64,
    layer: Option<String>,
) -> PyInsert {
    PyInsert::new(block_name, x, y, z, rotation, layer)
}

#[pyfunction]
#[pyo3(signature = (x=0.0, y=0.0, z=0.0, layer=None))]
pub fn make_point(x: f64, y: f64, z: f64, layer: Option<String>) -> PyPoint {
    PyPoint::new(x, y, z, layer)
}

#[pyfunction]
#[pyo3(signature = (start, end, layer=None))]
pub fn make_line(start: PyVector3, end: PyVector3, layer: Option<String>) -> PyLine {
    PyLine::new(Some(start), Some(end), layer)
}

#[pyfunction]
#[pyo3(signature = (center, radius=1.0, layer=None))]
pub fn make_circle(center: PyVector3, radius: f64, layer: Option<String>) -> PyCircle {
    PyCircle::new(Some(center), radius, layer)
}

#[pyfunction]
#[pyo3(signature = (center, radius=1.0, start_angle=0.0, end_angle=1.57079632679, layer=None))]
pub fn make_arc(
    center: PyVector3,
    radius: f64,
    start_angle: f64,
    end_angle: f64,
    layer: Option<String>,
) -> PyArc {
    PyArc::new(Some(center), radius, start_angle, end_angle, layer)
}

#[pyfunction]
#[pyo3(signature = (value=None, x=0.0, y=0.0, z=0.0, height=2.5, layer=None))]
pub fn make_text(
    value: Option<String>,
    x: f64,
    y: f64,
    z: f64,
    height: f64,
    layer: Option<String>,
) -> PyText {
    PyText::new(value, x, y, z, height, layer)
}

pub(crate) fn entity_to_py(py: Python, entity: &EntityType) -> PyResult<PyObject> {
    match entity {
        EntityType::Point(p) => {
            let loc = PyVector3 {
                x: p.location.x,
                y: p.location.y,
                z: p.location.z,
            };
            let py_point = PyPoint {
                location: loc,
                layer: p.common.layer.clone(),
            };
            Ok(py_point.into_py(py))
        }
        EntityType::Line(l) => {
            let start = PyVector3 {
                x: l.start.x,
                y: l.start.y,
                z: l.start.z,
            };
            let end = PyVector3 {
                x: l.end.x,
                y: l.end.y,
                z: l.end.z,
            };
            let py_line = PyLine {
                start,
                end,
                layer: l.common.layer.clone(),
            };
            Ok(py_line.into_py(py))
        }
        EntityType::Circle(c) => {
            let center = PyVector3 {
                x: c.center.x,
                y: c.center.y,
                z: c.center.z,
            };
            let py_circle = PyCircle {
                center,
                radius: c.radius,
                layer: c.common.layer.clone(),
            };
            Ok(py_circle.into_py(py))
        }
        EntityType::Arc(a) => {
            let center = PyVector3 {
                x: a.center.x,
                y: a.center.y,
                z: a.center.z,
            };
            let py_arc = PyArc {
                center,
                radius: a.radius,
                start_angle: a.start_angle,
                end_angle: a.end_angle,
                layer: a.common.layer.clone(),
            };
            Ok(py_arc.into_py(py))
        }
        EntityType::Text(t) => {
            let loc = PyVector3 {
                x: t.insertion_point.x,
                y: t.insertion_point.y,
                z: t.insertion_point.z,
            };
            let py_text = PyText {
                value: t.value.clone(),
                location: loc,
                height: t.height,
                layer: t.common.layer.clone(),
            };
            Ok(py_text.into_py(py))
        }
        EntityType::MText(m) => {
            let loc = PyVector3 {
                x: m.insertion_point.x,
                y: m.insertion_point.y,
                z: m.insertion_point.z,
            };
            let py_mtext = PyMText {
                value: m.value.clone(),
                insertion_point: loc,
                height: m.height,
                layer: m.common.layer.clone(),
            };
            Ok(py_mtext.into_py(py))
        }
        EntityType::LwPolyline(p) => {
            let verts: Vec<(f64, f64, f64)> = p
                .vertices
                .iter()
                .map(|v| (v.location.x, v.location.y, v.bulge))
                .collect();
            let py_poly = PyLwPolyline {
                vertices: verts,
                is_closed: p.is_closed,
                layer: p.common.layer.clone(),
            };
            Ok(py_poly.into_py(py))
        }
        EntityType::Insert(ins) => {
            let loc = PyVector3 {
                x: ins.insert_point.x,
                y: ins.insert_point.y,
                z: ins.insert_point.z,
            };
            let py_ins = PyInsert {
                block_name: ins.block_name.clone(),
                insertion_point: loc,
                rotation: ins.rotation,
                layer: ins.common.layer.clone(),
            };
            Ok(py_ins.into_py(py))
        }
        EntityType::Hatch(h) => {
            let boundary: Vec<(f64, f64)> = h
                .paths
                .first()
                .map(|p| {
                    p.edges
                        .iter()
                        .filter_map(|e| match e {
                            acadrust::entities::hatch::BoundaryEdge::Line(l) => {
                                Some((l.start.x, l.start.y))
                            }
                            _ => None,
                        })
                        .collect()
                })
                .unwrap_or_default();
            let py_hatch = PyHatch {
                boundary,
                is_solid: h.is_solid,
                layer: h.common.layer.clone(),
            };
            Ok(py_hatch.into_py(py))
        }
        EntityType::Dimension(d) => {
            use acadrust::entities::Dimension;
            match d {
                Dimension::Linear(dim) => {
                    let start = PyVector3 {
                        x: dim.first_point.x,
                        y: dim.first_point.y,
                        z: dim.first_point.z,
                    };
                    let end = PyVector3 {
                        x: dim.second_point.x,
                        y: dim.second_point.y,
                        z: dim.second_point.z,
                    };
                    let py_dim = PyDimension {
                        start,
                        end,
                        layer: dim.base.common.layer.clone(),
                    };
                    Ok(py_dim.into_py(py))
                }
                _ => {
                    let generic = PyEntity {
                        handle: d.base().common.handle.value(),
                        kind: entity_kind_name(d),
                        layer: d.base().common.layer.clone(),
                    };
                    Ok(generic.into_py(py))
                }
            }
        }
        EntityType::Leader(l) => {
            let vertices: Vec<PyVector3> = l
                .vertices
                .iter()
                .map(|v| PyVector3 { x: v.x, y: v.y, z: v.z })
                .collect();
            let py_leader = PyLeader {
                vertices,
                layer: l.common.layer.clone(),
            };
            Ok(py_leader.into_py(py))
        }
        EntityType::Viewport(vp) => {
            let center = PyVector3 {
                x: vp.center.x,
                y: vp.center.y,
                z: vp.center.z,
            };
            let py_vp = PyViewport {
                center,
                width: vp.width,
                height: vp.height,
                id: vp.id,
                layer: vp.common.layer.clone(),
            };
            Ok(py_vp.into_py(py))
        }
        EntityType::Spline(s) => {
            let control_points: Vec<PyVector3> = s
                .control_points
                .iter()
                .map(|p| PyVector3 { x: p.x, y: p.y, z: p.z })
                .collect();
            let py_spline = PySpline {
                control_points,
                knots: s.knots.clone(),
                degree: s.degree as i16,
                is_closed: s.flags.closed || s.flags.periodic,
                layer: s.common.layer.clone(),
            };
            Ok(py_spline.into_py(py))
        }
        _ => {
            let generic = PyEntity {
                handle: entity.common().handle.value(),
                kind: entity_kind_name(entity),
                layer: entity.common().layer.clone(),
            };
            Ok(generic.into_py(py))
        }
    }
}

pub(crate) fn py_to_entity_op(obj: &Bound<'_, PyAny>) -> PyResult<EntityOp> {
    if let Ok(point) = obj.extract::<PyPoint>() {
        use acadrust::entities::Point;
        use acadrust::types::Vector3;
        let mut p = Point::at(Vector3::new(point.location.x, point.location.y, point.location.z));
        p.common.layer = point.layer;
        return Ok(EntityOp::Add(EntityType::Point(p)));
    }
    if let Ok(line) = obj.extract::<PyLine>() {
        use acadrust::entities::Line;
        use acadrust::types::Vector3;
        let mut l = Line::from_points(
            Vector3::new(line.start.x, line.start.y, line.start.z),
            Vector3::new(line.end.x, line.end.y, line.end.z),
        );
        l.common.layer = line.layer;
        return Ok(EntityOp::Add(EntityType::Line(l)));
    }
    if let Ok(circle) = obj.extract::<PyCircle>() {
        use acadrust::entities::Circle;
        use acadrust::types::Vector3;
        let mut c = Circle {
            center: Vector3::new(circle.center.x, circle.center.y, circle.center.z),
            radius: circle.radius,
            ..Default::default()
        };
        c.common.layer = circle.layer;
        return Ok(EntityOp::Add(EntityType::Circle(c)));
    }
    if let Ok(arc) = obj.extract::<PyArc>() {
        use acadrust::entities::Arc;
        use acadrust::types::Vector3;
        let mut a = Arc {
            center: Vector3::new(arc.center.x, arc.center.y, arc.center.z),
            radius: arc.radius,
            start_angle: arc.start_angle,
            end_angle: arc.end_angle,
            ..Default::default()
        };
        a.common.layer = arc.layer;
        return Ok(EntityOp::Add(EntityType::Arc(a)));
    }
    if let Ok(text) = obj.extract::<PyText>() {
        use acadrust::entities::Text;
        use acadrust::types::Vector3;
        let mut t =
            Text::with_value(&text.value, Vector3::new(text.location.x, text.location.y, text.location.z))
                .with_height(text.height);
        t.common.layer = text.layer;
        return Ok(EntityOp::Add(EntityType::Text(t)));
    }
    if let Ok(mtext) = obj.extract::<PyMText>() {
        use acadrust::entities::MText;
        use acadrust::types::Vector3;
        let mut m = MText::new();
        m.value = mtext.value;
        m.insertion_point = Vector3::new(mtext.insertion_point.x, mtext.insertion_point.y, mtext.insertion_point.z);
        m.height = mtext.height;
        m.common.layer = mtext.layer;
        return Ok(EntityOp::Add(EntityType::MText(m)));
    }
    if let Ok(poly) = obj.extract::<PyLwPolyline>() {
        use acadrust::entities::{LwPolyline, LwVertex};
        use acadrust::types::Vector2;
        let mut p = LwPolyline::new();
        p.is_closed = poly.is_closed;
        p.vertices = poly
            .vertices
            .into_iter()
            .map(|(x, y, bulge)| {
                let mut v = LwVertex::new(Vector2::new(x, y));
                v.bulge = bulge;
                v
            })
            .collect();
        p.common.layer = poly.layer;
        return Ok(EntityOp::Add(EntityType::LwPolyline(p)));
    }
    if let Ok(insert) = obj.extract::<PyInsert>() {
        use acadrust::entities::Insert;
        use acadrust::types::Vector3;
        let mut ins = Insert::new(
            &insert.block_name,
            Vector3::new(insert.insertion_point.x, insert.insertion_point.y, insert.insertion_point.z),
        );
        ins.rotation = insert.rotation;
        ins.common.layer = insert.layer;
        return Ok(EntityOp::Add(EntityType::Insert(ins)));
    }
    if let Ok(hatch) = obj.extract::<PyHatch>() {
        use acadrust::entities::hatch::{BoundaryEdge, BoundaryPath, LineEdge};
        use acadrust::entities::Hatch;
        use acadrust::types::Vector2;
        let mut h = Hatch::new();
        h.is_solid = hatch.is_solid;
        if !hatch.boundary.is_empty() {
            let mut path = BoundaryPath::new();
            let n = hatch.boundary.len();
            for i in 0..n {
                let (x0, y0) = hatch.boundary[i];
                let (x1, y1) = hatch.boundary[(i + 1) % n];
                path.edges.push(BoundaryEdge::Line(LineEdge {
                    start: Vector2::new(x0, y0),
                    end: Vector2::new(x1, y1),
                }));
            }
            h.paths.push(path);
        }
        h.common.layer = hatch.layer;
        return Ok(EntityOp::Add(EntityType::Hatch(h)));
    }
    if let Ok(dimension) = obj.extract::<PyDimension>() {
        use acadrust::entities::{Dimension, DimensionLinear};
        use acadrust::types::Vector3;
        let mut d = DimensionLinear::new(
            Vector3::new(dimension.start.x, dimension.start.y, dimension.start.z),
            Vector3::new(dimension.end.x, dimension.end.y, dimension.end.z),
        );
        d.base.common.layer = dimension.layer;
        return Ok(EntityOp::Add(EntityType::Dimension(Dimension::Linear(d))));
    }
    if let Ok(leader) = obj.extract::<PyLeader>() {
        use acadrust::entities::Leader;
        use acadrust::types::Vector3;
        let mut l = Leader::new();
        l.vertices = leader
            .vertices
            .into_iter()
            .map(|v| Vector3::new(v.x, v.y, v.z))
            .collect();
        l.common.layer = leader.layer;
        return Ok(EntityOp::Add(EntityType::Leader(l)));
    }
    if let Ok(viewport) = obj.extract::<PyViewport>() {
        use acadrust::entities::Viewport;
        use acadrust::types::Vector3;
        let mut vp = Viewport::new();
        vp.center = Vector3::new(viewport.center.x, viewport.center.y, viewport.center.z);
        vp.width = viewport.width;
        vp.height = viewport.height;
        vp.id = viewport.id;
        vp.common.layer = viewport.layer;
        return Ok(EntityOp::Add(EntityType::Viewport(vp)));
    }
    if let Ok(spline) = obj.extract::<PySpline>() {
        use acadrust::entities::Spline;
        use acadrust::types::Vector3;
        let mut s = Spline::new();
        s.degree = spline.degree as i32;
        s.control_points = spline
            .control_points
            .into_iter()
            .map(|p| Vector3::new(p.x, p.y, p.z))
            .collect();
        s.knots = spline.knots;
        s.flags.closed = spline.is_closed;
        s.common.layer = spline.layer;
        return Ok(EntityOp::Add(EntityType::Spline(s)));
    }
    Err(pyo3::exceptions::PyTypeError::new_err(
        "entity type not supported",
    ))
}
