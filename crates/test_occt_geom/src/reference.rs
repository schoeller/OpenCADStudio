//! Analytic reference metrics for the primitives supported by [`OcsKernel`].

use crate::geom::Primitive;
use crate::metrics::{ExpectedMetrics, GeomMetrics};
use std::f64::consts::PI;

/// Expected metrics for a primitive, derived from closed-form geometry.
///
/// These are used as the ground truth when the optional `cadrum` reference
/// kernel is not available.  They are intentionally independent of
/// tessellation quality.
pub fn analytic_primitive(p: &Primitive) -> ExpectedMetrics {
    match *p {
        Primitive::Box {
            center: [cx, cy, cz],
            length,
            width,
            height,
        } => ExpectedMetrics::default()
            .volume(length * width * height)
            .surface_area(2.0 * (length * width + length * height + width * height))
            .centroid([cx, cy, cz])
            .bbox_min([cx - length / 2.0, cy - width / 2.0, cz - height / 2.0])
            .bbox_max([cx + length / 2.0, cy + width / 2.0, cz + height / 2.0]),

        Primitive::Wedge {
            origin: [ox, oy, oz],
            length,
            width,
            height,
        } => {
            let volume = 0.5 * length * width * height;
            let hypot = (length * length + height * height).sqrt();
            let area = length * height + length * width + height * width + width * hypot;
            ExpectedMetrics::default()
                .volume(volume)
                .surface_area(area)
                .centroid([ox + length / 3.0, oy + width / 2.0, oz + height / 3.0])
                .bbox_min([ox, oy, oz])
                .bbox_max([ox + length, oy + width, oz + height])
        }

        Primitive::Cylinder {
            center: [cx, cy, cz],
            radius,
            height,
        } => ExpectedMetrics::default()
            .volume(PI * radius * radius * height)
            .surface_area(2.0 * PI * radius * (radius + height))
            .centroid([cx, cy, cz + height / 2.0])
            .bbox_min([cx - radius, cy - radius, cz])
            .bbox_max([cx + radius, cy + radius, cz + height]),

        Primitive::Cone {
            center: [cx, cy, cz],
            radius,
            height,
        } => {
            let slant = (radius * radius + height * height).sqrt();
            ExpectedMetrics::default()
                .volume(PI * radius * radius * height / 3.0)
                .surface_area(PI * radius * (radius + slant))
                .centroid([cx, cy, cz + height / 4.0])
                .bbox_min([cx - radius, cy - radius, cz])
                .bbox_max([cx + radius, cy + radius, cz + height])
        }

        Primitive::Sphere {
            center: [cx, cy, cz],
            radius,
        } => ExpectedMetrics::default()
            .volume(4.0 / 3.0 * PI * radius.powi(3))
            .surface_area(4.0 * PI * radius * radius)
            .centroid([cx, cy, cz])
            .bbox_min([cx - radius, cy - radius, cz - radius])
            .bbox_max([cx + radius, cy + radius, cz + radius]),

        Primitive::Torus {
            center: [cx, cy, cz],
            major,
            minor,
        } => ExpectedMetrics::default()
            .volume(2.0 * PI * PI * major * minor * minor)
            .surface_area(4.0 * PI * PI * major * minor)
            .centroid([cx, cy, cz])
            .bbox_min([cx - major - minor, cy - major - minor, cz - minor])
            .bbox_max([cx + major + minor, cy + major + minor, cz + minor]),
    }
}

/// Build an [`ExpectedMetrics`] from an analytic [`GeomMetrics`] value.
pub fn from_geom(metrics: &GeomMetrics) -> ExpectedMetrics {
    ExpectedMetrics::exact(metrics)
}
