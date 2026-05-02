use acadrust::types::{Transform, Vector3};
use glam::Vec3;

use crate::command::EntityTransform;

#[inline]
fn to_v3(v: Vec3) -> Vector3 {
    Vector3::new(v.x as f64, v.y as f64, v.z as f64)
}

pub fn apply_standard_transform<T>(entity: &mut T, center: Vec3, angle_rad: f32)
where
    T: acadrust::Entity,
{
    let z_axis = Vector3::new(0.0, 0.0, 1.0);
    let t = Transform::from_translation(to_v3(-center))
        .then(&Transform::from_rotation(z_axis, angle_rad as f64))
        .then(&Transform::from_translation(to_v3(center)));
    entity.apply_transform(&t);
}

pub fn apply_standard_scale<T>(entity: &mut T, center: Vec3, factor: f32)
where
    T: acadrust::Entity,
{
    let s = factor as f64;
    let t = Transform::from_scaling_with_origin(Vector3::new(s, s, s), to_v3(center));
    entity.apply_transform(&t);
}

pub fn apply_standard_entity_transform<T, F>(entity: &mut T, t: &EntityTransform, mirror: F)
where
    T: acadrust::Entity,
    F: FnOnce(&mut T, Vec3, Vec3),
{
    match t {
        EntityTransform::Translate(d) => entity.translate(to_v3(*d)),
        EntityTransform::Rotate { center, angle_rad } => {
            apply_standard_transform(entity, *center, *angle_rad)
        }
        EntityTransform::Scale { center, factor } => apply_standard_scale(entity, *center, *factor),
        EntityTransform::Mirror { p1, p2 } => mirror(entity, *p1, *p2),
    }
}

pub fn reflect_xy_point(x: &mut f64, y: &mut f64, p1: Vec3, p2: Vec3) {
    let ax = (p2.x - p1.x) as f64;
    let ay = (p2.y - p1.y) as f64;
    let len2 = ax * ax + ay * ay;
    if len2 < 1e-12 {
        return;
    }
    let rx = *x - p1.x as f64;
    let ry = *y - p1.y as f64;
    let dot = rx * ax + ry * ay;
    let mx = 2.0 * dot * ax / len2 - rx;
    let my = 2.0 * dot * ay / len2 - ry;
    *x = p1.x as f64 + mx;
    *y = p1.y as f64 + my;
}

pub fn mirror_xy_line(line: &mut acadrust::entities::Line, p1: Vec3, p2: Vec3) {
    reflect_xy_point(&mut line.start.x, &mut line.start.y, p1, p2);
    reflect_xy_point(&mut line.end.x, &mut line.end.y, p1, p2);
}

/// DXF arbitrary-axis algorithm — returns the OCS X and Y basis vectors in WCS
/// for a given entity normal vector.
///
/// Returns (Ax, Ay) each as (x, y, z) tuples.  When normal ≈ (0,0,1) the
/// function returns the standard basis ((1,0,0), (0,1,0)) without any
/// cross-product computation.
pub fn ocs_axes(normal: (f64, f64, f64)) -> ((f64, f64, f64), (f64, f64, f64)) {
    let (nx, ny, nz) = normal;
    // Fast path: normal is effectively Z-up, OCS = WCS.
    if nx.abs() < 1e-10 && ny.abs() < 1e-10 && (nz - 1.0).abs() < 1e-10 {
        return ((1.0, 0.0, 0.0), (0.0, 1.0, 0.0));
    }
    // Arbitrary-axis algorithm (DXF spec).
    // When |Nx| < 1/64 and |Ny| < 1/64 use Wy=(0,1,0)×N, else Wx=(1,0,0)×N.
    let (ax_x, ax_y, ax_z) = if nx.abs() < 1.0 / 64.0 && ny.abs() < 1.0 / 64.0 {
        // (0,1,0) × (nx,ny,nz) = (nz, 0, -nx)
        let (x, y, z) = (nz, 0.0, -nx);
        let len = (x * x + y * y + z * z).sqrt().max(1e-12);
        (x / len, y / len, z / len)
    } else {
        // (1,0,0) × (nx,ny,nz) = (0, -nz, ny)
        let (x, y, z) = (0.0, -nz, ny);
        let len = (x * x + y * y + z * z).sqrt().max(1e-12);
        (x / len, y / len, z / len)
    };
    // Ay = N × Ax
    let ay_x = ny * ax_z - nz * ax_y;
    let ay_y = nz * ax_x - nx * ax_z;
    let ay_z = nx * ax_y - ny * ax_x;
    ((ax_x, ax_y, ax_z), (ay_x, ay_y, ay_z))
}

/// Transform a single OCS point to WCS using the given normal vector.
#[inline]
pub fn ocs_point_to_wcs(
    ocs: (f64, f64, f64),
    normal: (f64, f64, f64),
) -> (f64, f64, f64) {
    let (ax, ay) = ocs_axes(normal);
    let (ox, oy, oz) = ocs;
    let (nx, ny, nz) = normal;
    (
        ox * ax.0 + oy * ay.0 + oz * nx,
        ox * ax.1 + oy * ay.1 + oz * ny,
        ox * ax.2 + oy * ay.2 + oz * nz,
    )
}
