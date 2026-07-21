//! Pythonic entity wrappers and constructors.

use pyo3::prelude::*;

use acadrust::{EntityType, Handle};
use ocs_plugin_api::shm::EntityOp;

use crate::geometry::PyVector3;

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
        let mut queue = crate::document::open_queue()?;
        queue
            .push(&EntityOp::Remove(Handle::new(self.handle)))
            .map_err(crate::document::queue_err)?;
        Ok(())
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
        _ => {
            let generic = PyEntity {
                handle: entity.common().handle.value(),
                kind: format!("{:?}", entity),
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
    Err(pyo3::exceptions::PyTypeError::new_err(
        "entity type not supported",
    ))
}
