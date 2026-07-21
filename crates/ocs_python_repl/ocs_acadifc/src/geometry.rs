//! Geometry helpers exposed to Python.

use pyo3::prelude::*;

#[pyclass(name = "Vector3")]
#[derive(Clone, Copy, Debug)]
pub struct PyVector3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

#[pymethods]
impl PyVector3 {
    #[new]
    fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }

    #[getter]
    fn x(&self) -> f64 {
        self.x
    }

    #[getter]
    fn y(&self) -> f64 {
        self.y
    }

    #[getter]
    fn z(&self) -> f64 {
        self.z
    }

    fn __repr__(&self) -> String {
        format!("Vector3({:.3}, {:.3}, {:.3})", self.x, self.y, self.z)
    }
}

#[pyclass(name = "Color")]
#[derive(Clone, Copy, Debug)]
pub struct PyColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

#[pymethods]
impl PyColor {
    #[new]
    fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    #[getter]
    fn r(&self) -> u8 {
        self.r
    }

    #[getter]
    fn g(&self) -> u8 {
        self.g
    }

    #[getter]
    fn b(&self) -> u8 {
        self.b
    }
}
