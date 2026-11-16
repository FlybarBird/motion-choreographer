//! The model as the program keeps it: corner positions, triangles, one colour per triangle. Also its
//! compact file form and the search structure behind clicking on the model.

use crate::math::{add, cross, dot, scale, sub};
use crate::{too_many_triangles, MAX_TRIANGLES};

/// Colour of faces whose file gives them none.
pub const DEFAULT_COLOR: [u8; 3] = [170, 170, 170];

const MAGIC: &[u8; 4] = b"PTZM";
const VERSION: u32 = 1;
const HEADER_LEN: usize = 16;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mesh {
    pub positions: Vec<[f32; 3]>,
    pub triangles: Vec<[u32; 3]>,
    /// One per triangle, same length as `triangles`.
    pub colors: Vec<[u8; 3]>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

impl Bounds {
    pub fn size(&self) -> [f32; 3] {
        sub(self.max, self.min)
    }

    pub fn center(&self) -> [f32; 3] {
        scale(add(self.min, self.max), 0.5)
    }
}

/// p' = m * p + t  (rows of m dotted with p)
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub m: [[f32; 3]; 3],
    pub t: [f32; 3],
}

impl Transform {
    pub const IDENTITY: Transform = Transform { m: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], t: [0.0; 3] };

    pub fn scale(s: f32) -> Transform {
        Transform { m: [[s, 0.0, 0.0], [0.0, s, 0.0], [0.0, 0.0, s]], t: [0.0; 3] }
    }

    pub fn translation(t: [f32; 3]) -> Transform {
        Transform { t, ..Transform::IDENTITY }
    }

    /// Counter-clockwise seen from +Z.
    pub fn rotation_z(degrees: f32) -> Transform {
        let (sin, cos) = degrees.to_radians().sin_cos();
        Transform { m: [[cos, -sin, 0.0], [sin, cos, 0.0], [0.0, 0.0, 1.0]], t: [0.0; 3] }
    }

    /// (x, y, z) -> (x, -z, y): a quarter turn about X, so nothing is mirrored.
    pub fn y_up_to_z_up() -> Transform {
        Transform { m: [[1.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]], t: [0.0; 3] }
    }

    /// (x, y, z) -> (x, z, -y): undoes `y_up_to_z_up`.
    pub fn z_up_to_y_up() -> Transform {
        Transform { m: [[1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, -1.0, 0.0]], t: [0.0; 3] }
    }

    /// The transform that applies `self` first and `next` after it.
    pub fn then(&self, next: &Transform) -> Transform {
        let column = |c: usize| [self.m[0][c], self.m[1][c], self.m[2][c]];
        let m = [0, 1, 2].map(|r| [0, 1, 2].map(|c| dot(next.m[r], column(c))));
        Transform { m, t: next.apply(self.t) }
    }

    pub fn apply(&self, p: [f32; 3]) -> [f32; 3] {
        [0, 1, 2].map(|r| dot(self.m[r], p) + self.t[r])
    }
}

pub(crate) fn determinant(m: &[[f32; 3]; 3]) -> f32 {
    dot(m[0], cross(m[1], m[2]))
}

impl Mesh {
    /// None when there are no positions.
    pub fn bounds(&self) -> Option<Bounds> {
        let first = *self.positions.first()?;
        let mut bounds = Bounds { min: first, max: first };
        for p in &self.positions {
            for (k, &value) in p.iter().enumerate() {
                bounds.min[k] = bounds.min[k].min(value);
                bounds.max[k] = bounds.max[k].max(value);
            }
        }
        Some(bounds)
    }

    /// A mirroring transform also reverses each triangle, so the faces keep pointing outward.
    pub fn transformed(&self, t: &Transform) -> Mesh {
        let mirrored = determinant(&t.m) < 0.0;
        Mesh {
            positions: self.positions.iter().map(|&p| t.apply(p)).collect(),
            triangles: self.triangles.iter().map(|&[a, b, c]| if mirrored { [a, c, b] } else { [a, b, c] }).collect(),
            colors: self.colors.clone(),
        }
    }

    pub fn append(&mut self, other: &Mesh) {
        let base = self.positions.len() as u32;
        self.positions.extend_from_slice(&other.positions);
        self.triangles.extend(other.triangles.iter().map(|t| t.map(|i| i.wrapping_add(base))));
        self.colors.extend_from_slice(&other.colors);
    }

    pub fn push_triangle(&mut self, a: [f32; 3], b: [f32; 3], c: [f32; 3], color: [u8; 3]) {
        let base = self.positions.len() as u32;
        self.positions.extend([a, b, c]);
        self.triangles.push([base, base + 1, base + 2]);
        self.colors.push(color);
    }

    /// Corners in order around the quad.
    pub fn push_quad(&mut self, corners: [[f32; 3]; 4], color: [u8; 3]) {
        let base = self.positions.len() as u32;
        self.positions.extend(corners);
        self.triangles.extend([[base, base + 1, base + 2], [base, base + 2, base + 3]]);
        self.colors.extend([color; 2]);
    }

    /// A box between two opposite corners: 8 positions, 12 triangles, each counter-clockwise seen from outside.
    pub fn push_cuboid(&mut self, min: [f32; 3], max: [f32; 3], color: [u8; 3]) {
        // Corner `i` takes its x from `max` when bit 0 of `i` is set, y for bit 1, z for bit 2.
        const FACES: [[u32; 4]; 6] = [[0, 2, 3, 1], [4, 5, 7, 6], [0, 1, 5, 4], [2, 6, 7, 3], [0, 4, 6, 2], [1, 3, 7, 5]];
        let base = self.positions.len() as u32;
        self.positions.extend((0..8).map(|i| [0, 1, 2].map(|k| if i >> k & 1 == 1 { max[k] } else { min[k] })));
        for [a, b, c, d] in FACES {
            self.triangles.extend([[base + a, base + b, base + c], [base + a, base + c, base + d]]);
            self.colors.extend([color; 2]);
        }
    }

    /// Checks every index is in range, colors.len() == triangles.len(), and every coordinate is finite.
    pub fn validate(&self) -> Result<(), String> {
        if self.colors.len() != self.triangles.len() {
            return Err(format!(
                "The model is damaged: it has {} triangles but colours for {}",
                self.triangles.len(),
                self.colors.len()
            ));
        }
        if let Some(at) = self.positions.iter().position(|p| !is_finite(p)) {
            return Err(format!("The model is damaged: the position of vertex {} is not a usable number", at + 1));
        }
        let count = self.positions.len();
        for (at, triangle) in self.triangles.iter().enumerate() {
            if let Some(index) = triangle.iter().find(|&&index| index as usize >= count) {
                return Err(format!(
                    "The model is damaged: triangle {} refers to vertex {}, but the model has only {count}",
                    at + 1,
                    *index as u64 + 1
                ));
            }
        }
        Ok(())
    }

    /// The `.ptzmesh` form, little-endian: `PTZM`, version, vertex count, triangle count (u32 each), then
    /// the positions (3 x f32), the triangles (3 x u32) and the colours (3 x u8).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(HEADER_LEN + self.positions.len() * 12 + self.triangles.len() * 15);
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&VERSION.to_le_bytes());
        bytes.extend_from_slice(&(self.positions.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&(self.triangles.len() as u32).to_le_bytes());
        bytes.extend(self.positions.iter().flatten().flat_map(|v| v.to_le_bytes()));
        bytes.extend(self.triangles.iter().flatten().flat_map(|v| v.to_le_bytes()));
        for at in 0..self.triangles.len() {
            bytes.extend_from_slice(self.colors.get(at).unwrap_or(&DEFAULT_COLOR));
        }
        bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Mesh, String> {
        if !bytes.starts_with(MAGIC) {
            return Err("This is not a PTZ mesh file".into());
        }
        if bytes.len() < HEADER_LEN {
            return Err("The mesh file is cut short: its heading is incomplete".into());
        }
        let (header, body) = bytes.split_at(HEADER_LEN);
        let version = le_u32(&header[4..]);
        if version != VERSION {
            return Err(format!(
                "This mesh file is version {version}, but this program reads version {VERSION}. It may come from a newer release"
            ));
        }
        let (vertices, triangles) = (le_u32(&header[8..]) as usize, le_u32(&header[12..]) as usize);
        if triangles > MAX_TRIANGLES {
            return Err(too_many_triangles());
        }
        // In u64 so that no pair of counts can overflow, and compared before anything is allocated.
        let expected = HEADER_LEN as u64 + vertices as u64 * 12 + triangles as u64 * 15;
        if bytes.len() as u64 != expected {
            return Err(format!("The mesh file is damaged: it should be {expected} bytes long but is {}", bytes.len()));
        }
        let (positions, rest) = body.split_at(vertices * 12);
        let (indices, colors) = rest.split_at(triangles * 12);
        let mesh = Mesh {
            positions: positions
                .as_chunks::<12>()
                .0
                .iter()
                .map(|p| [0, 4, 8].map(|at| f32::from_bits(le_u32(&p[at..]))))
                .collect(),
            triangles: indices.as_chunks::<12>().0.iter().map(|t| [0, 4, 8].map(|at| le_u32(&t[at..]))).collect(),
            colors: colors.as_chunks::<3>().0.iter().map(|c| [c[0], c[1], c[2]]).collect(),
        };
        mesh.validate()?;
        Ok(mesh)
    }

    /// The corners of triangle `at`, when its indices are in range and its positions finite.
    pub(crate) fn corners(&self, at: usize) -> Option<[[f32; 3]; 3]> {
        let [a, b, c] = *self.triangles.get(at)?;
        let corner = |index: u32| self.positions.get(index as usize).copied().filter(is_finite);
        Some([corner(a)?, corner(b)?, corner(c)?])
    }
}

fn is_finite(p: &[f32; 3]) -> bool {
    p.iter().all(|v| v.is_finite())
}

/// The first four bytes of `bytes` as a little-endian number; callers pass slices they have already measured.
fn le_u32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub distance: f32,
    pub triangle: u32,
    pub point: [f32; 3],
}

/// Finds what a ray hits, for clicking on the model. Built once per model.
///
/// The model's box is cut into a grid of cells, each listing the triangles that reach into it; a ray then
/// only looks at the triangles of the cells it passes through, nearest cell first.
pub struct Picker {
    triangles: Vec<[[f32; 3]; 3]>,
    min: [f32; 3],
    cell: [f32; 3],
    dims: [usize; 3],
    /// `items[starts[c]..starts[c + 1]]` are the triangles of cell `c`.
    starts: Vec<u32>,
    items: Vec<u32>,
}

const MAX_CELLS_PER_AXIS: usize = 128;

impl Picker {
    pub fn new(mesh: &Mesh) -> Picker {
        // Triangles that cannot be hit stay in the list as NaN so that hit numbers match the mesh.
        let triangles: Vec<[[f32; 3]; 3]> =
            (0..mesh.triangles.len()).map(|at| mesh.corners(at).unwrap_or([[f32::NAN; 3]; 3])).collect();
        let usable = || triangles.iter().enumerate().filter(|(_, t)| t[0][0].is_finite());
        let mut picker =
            Picker { triangles: Vec::new(), min: [0.0; 3], cell: [1.0; 3], dims: [1; 3], starts: vec![0, 0], items: Vec::new() };
        let count = usable().count();
        if count == 0 {
            return picker;
        }

        let (mut min, mut max) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
        for p in usable().flat_map(|(_, t)| t) {
            for k in 0..3 {
                min[k] = min[k].min(p[k]);
                max[k] = max[k].max(p[k]);
            }
        }
        // A little room all round, so a flat model still has a box with some thickness.
        let size = sub(max, min);
        let margin = (size[0].max(size[1]).max(size[2]) * 1e-4).max(1e-4);
        picker.min = min.map(|v| v - margin);
        let size = size.map(|v| v + 2.0 * margin);

        // About four cells per triangle, roughly cubic. Triangles that are large for their cells would be
        // listed in very many of them, so the grid is coarsened until the lists are a sensible size.
        let cells = (count * 4).min(1 << 21) as f32;
        let side = (size[0] * size[1] * size[2] / cells).cbrt();
        picker.dims = size.map(|v| ((v / side).ceil() as usize).clamp(1, MAX_CELLS_PER_AXIS));
        loop {
            picker.cell = [0, 1, 2].map(|k| size[k] / picker.dims[k] as f32);
            let listed: u64 =
                usable().map(|(_, t)| picker.cells_of(t).iter().map(|(a, b)| (b - a + 1) as u64).product::<u64>()).sum();
            if listed <= 8 * count as u64 + 1024 || picker.dims == [1; 3] {
                break;
            }
            picker.dims = picker.dims.map(|d| (d / 2).max(1));
        }

        let cell_count = picker.dims[0] * picker.dims[1] * picker.dims[2];
        let mut starts = vec![0u32; cell_count + 1];
        for (_, t) in usable() {
            picker.for_each_cell(t, |cell| starts[cell + 1] += 1);
        }
        for cell in 0..cell_count {
            starts[cell + 1] += starts[cell];
        }
        let mut next = starts.clone();
        let mut items = vec![0u32; starts[cell_count] as usize];
        for (at, t) in usable() {
            picker.for_each_cell(t, |cell| {
                items[next[cell] as usize] = at as u32;
                next[cell] += 1;
            });
        }
        Picker { triangles, starts, items, ..picker }
    }

    /// Nearest triangle hit by the ray (direction need not be unit length; distance is in units of world length). Two-sided.
    pub fn hit(&self, origin: [f32; 3], direction: [f32; 3]) -> Option<Hit> {
        let length = dot(direction, direction).sqrt();
        if self.items.is_empty() || !(length > 0.0 && length.is_finite()) || !is_finite(&origin) {
            return None;
        }
        let d = scale(direction, 1.0 / length);

        // The stretch of the ray that lies inside the grid's box.
        let (mut enter, mut leave) = (0.0f32, f32::INFINITY);
        for k in 0..3 {
            let (low, high) = (self.min[k], self.min[k] + self.cell[k] * self.dims[k] as f32);
            if d[k] == 0.0 {
                if origin[k] < low || origin[k] > high {
                    return None;
                }
            } else {
                let (a, b) = ((low - origin[k]) / d[k], (high - origin[k]) / d[k]);
                enter = enter.max(a.min(b));
                leave = leave.min(a.max(b));
            }
        }
        if enter > leave {
            return None;
        }

        let start = add(origin, scale(d, enter));
        let mut cell = [0, 1, 2].map(|k| self.cell_index(start[k], k));
        let mut best: Option<(f32, u32)> = None;
        loop {
            // Where the ray leaves this cell, and through which pair of faces.
            let (mut exit, mut axis) = (f32::INFINITY, 0);
            for k in 0..3 {
                if d[k] != 0.0 {
                    let face = self.min[k] + self.cell[k] * (cell[k] + usize::from(d[k] > 0.0)) as f32;
                    let t = (face - origin[k]) / d[k];
                    if t < exit {
                        (exit, axis) = (t, k);
                    }
                }
            }
            let at = cell[0] + self.dims[0] * (cell[1] + self.dims[1] * cell[2]);
            for &item in &self.items[self.starts[at] as usize..self.starts[at + 1] as usize] {
                if let Some(t) = ray_triangle(origin, d, &self.triangles[item as usize]) {
                    if best.is_none_or(|(nearest, _)| t < nearest) {
                        best = Some((t, item));
                    }
                }
            }
            // A hit beyond this cell may still be beaten by a triangle listed only in a later cell.
            if best.is_some_and(|(nearest, _)| nearest <= exit) {
                break;
            }
            if d[axis] > 0.0 {
                cell[axis] += 1;
                if cell[axis] >= self.dims[axis] {
                    break;
                }
            } else if cell[axis] == 0 {
                break;
            } else {
                cell[axis] -= 1;
            }
        }
        best.map(|(distance, triangle)| Hit { distance, triangle, point: add(origin, scale(d, distance)) })
    }

    /// The cell along axis `k` that holds coordinate `v`; coordinates outside the grid land in its outermost cells.
    fn cell_index(&self, v: f32, k: usize) -> usize {
        (((v - self.min[k]) / self.cell[k]) as usize).min(self.dims[k] - 1)
    }

    /// First and last cell along each axis that the triangle's box reaches into.
    fn cells_of(&self, t: &[[f32; 3]; 3]) -> [(usize, usize); 3] {
        [0, 1, 2].map(|k| {
            // Slightly generous, so a hit right on a cell wall is found from either side.
            let slack = self.cell[k] * 1e-3;
            let (low, high) = (t[0][k].min(t[1][k]).min(t[2][k]), t[0][k].max(t[1][k]).max(t[2][k]));
            (self.cell_index(low - slack, k), self.cell_index(high + slack, k))
        })
    }

    fn for_each_cell(&self, t: &[[f32; 3]; 3], mut visit: impl FnMut(usize)) {
        let [x, y, z] = self.cells_of(t);
        for cz in z.0..=z.1 {
            for cy in y.0..=y.1 {
                for cx in x.0..=x.1 {
                    visit(cx + self.dims[0] * (cy + self.dims[1] * cz));
                }
            }
        }
    }
}

/// Distance along the unit ray to the triangle, from either side (Möller–Trumbore).
pub(crate) fn ray_triangle(origin: [f32; 3], d: [f32; 3], [a, b, c]: &[[f32; 3]; 3]) -> Option<f32> {
    let (edge1, edge2) = (sub(*b, *a), sub(*c, *a));
    let p = cross(d, edge2);
    let det = dot(edge1, p);
    if det == 0.0 {
        return None;
    }
    let s = sub(origin, *a);
    let u = dot(s, p) / det;
    let q = cross(s, edge1);
    let v = dot(d, q) / det;
    let t = dot(edge2, q) / det;
    (u >= 0.0 && v >= 0.0 && u + v <= 1.0 && t >= 0.0 && t.is_finite()).then_some(t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::normalize;
    use crate::testing::{damage, Rng};

    fn cube() -> Mesh {
        let mut mesh = Mesh::default();
        mesh.push_cuboid([-1.0, -2.0, -3.0], [1.0, 2.0, 3.0], [200, 100, 50]);
        mesh
    }

    fn close(a: [f32; 3], b: [f32; 3]) -> bool {
        (0..3).all(|k| (a[k] - b[k]).abs() < 1e-5)
    }

    #[test]
    fn bytes_round_trip_exactly() {
        let mut mesh = cube();
        mesh.push_triangle([0.1, 0.2, 0.3], [1e-20, -1e20, 7.0], [0.0, -0.0, 3.5], [1, 2, 3]);
        let bytes = mesh.to_bytes();
        assert_eq!(bytes.len(), 16 + 11 * 12 + 13 * 15);
        assert_eq!(Mesh::from_bytes(&bytes).unwrap(), mesh);
        assert_eq!(Mesh::from_bytes(&Mesh::default().to_bytes()).unwrap(), Mesh::default());
    }

    #[test]
    fn damaged_bytes_are_refused() {
        let good = cube().to_bytes();
        let with = |change: &dyn Fn(&mut Vec<u8>)| {
            let mut bytes = good.clone();
            change(&mut bytes);
            Mesh::from_bytes(&bytes).unwrap_err()
        };
        assert_eq!(with(&|b| b[0] = b'X'), "This is not a PTZ mesh file");
        assert!(with(&|b| b[4] = 2).contains("version 2"));
        assert!(with(&|b| b.truncate(b.len() - 1)).contains("should be 292 bytes long but is 291"));
        assert!(with(&|b| b.push(0)).contains("should be 292 bytes long but is 293"));
        assert!(with(&|b| b.truncate(10)).contains("cut short"));
        let first_index = 16 + 8 * 12;
        assert!(with(&|b| b[first_index] = 8).contains("refers to vertex 9, but the model has only 8"));
        assert!(with(&|b| b[16..20].copy_from_slice(&f32::NAN.to_le_bytes())).contains("vertex 1 is not a usable number"));
        assert!(with(&|b| b[28..32].copy_from_slice(&f32::INFINITY.to_le_bytes())).contains("vertex 2"));
        // Counts that would need more memory than exists are refused by their size alone.
        assert!(with(&|b| b[8..12].copy_from_slice(&u32::MAX.to_le_bytes())).contains("should be"));
        assert!(with(&|b| b[12..16].copy_from_slice(&u32::MAX.to_le_bytes())).contains("1.5 million"));
        assert!(with(&|b| b[12..16].copy_from_slice(&(MAX_TRIANGLES as u32).to_le_bytes())).contains("should be"));
    }

    #[test]
    fn validate_names_what_is_wrong() {
        let mut mesh = cube();
        assert_eq!(mesh.validate(), Ok(()));
        mesh.colors.pop();
        assert!(mesh.validate().unwrap_err().contains("12 triangles but colours for 11"));
        let mut mesh = cube();
        mesh.triangles[3] = [0, 1, 99];
        assert!(mesh.validate().unwrap_err().contains("triangle 4 refers to vertex 100"));
        let mut mesh = cube();
        mesh.positions[2][1] = f32::NEG_INFINITY;
        assert!(mesh.validate().unwrap_err().contains("vertex 3"));
    }

    #[test]
    fn damaged_mesh_files_never_panic() {
        let good = cube().to_bytes();
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        for round in 0..4000 {
            let mut bytes = damage(&good, &mut rng);
            if round % 4 == 0 && bytes.len() >= 16 {
                let field = 4 + 4 * rng.below(3);
                let value = [0, 1, 8, 12, 13, u32::MAX, rng.next() as u32][rng.below(7)];
                bytes[field..field + 4].copy_from_slice(&value.to_le_bytes());
            }
            if let Ok(mesh) = Mesh::from_bytes(&bytes) {
                assert_eq!(mesh.validate(), Ok(()));
            }
        }
    }

    #[test]
    fn transforms_follow_their_conventions() {
        let up = Transform::y_up_to_z_up();
        assert_eq!(up.apply([0.0, 1.0, 0.0]), [0.0, 0.0, 1.0]);
        assert_eq!(up.apply([1.0, 2.0, 3.0]), [1.0, -3.0, 2.0]);
        // x cross y = z before, and still after: nothing was mirrored.
        let [x, y, z] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]].map(|axis| up.apply(axis));
        assert_eq!(cross(x, y), z);
        assert_eq!(determinant(&up.m), 1.0);
        assert_eq!(up.then(&Transform::z_up_to_y_up()), Transform::IDENTITY);

        assert!(close(Transform::rotation_z(90.0).apply([1.0, 0.0, 0.0]), [0.0, 1.0, 0.0]));
        assert!(close(Transform::rotation_z(90.0).apply([0.0, 1.0, 5.0]), [-1.0, 0.0, 5.0]));
        assert_eq!(Transform::scale(2.5).apply([1.0, -2.0, 4.0]), [2.5, -5.0, 10.0]);
        assert_eq!(Transform::translation([1.0, 2.0, 3.0]).apply([1.0, 1.0, 1.0]), [2.0, 3.0, 4.0]);
        assert_eq!(Transform::IDENTITY.apply([7.0, 8.0, 9.0]), [7.0, 8.0, 9.0]);

        // `then` applies the left transform first: scaling then moving differs from moving then scaling.
        let (grow, shift) = (Transform::scale(2.0), Transform::translation([1.0, 0.0, 0.0]));
        assert_eq!(grow.then(&shift).apply([1.0, 1.0, 1.0]), [3.0, 2.0, 2.0]);
        assert_eq!(shift.then(&grow).apply([1.0, 1.0, 1.0]), [4.0, 2.0, 2.0]);
        let (turn, p) = (Transform::rotation_z(30.0), [0.3, -1.2, 2.0]);
        assert!(close(turn.then(&shift).then(&up).apply(p), up.apply(shift.apply(turn.apply(p)))));
    }

    #[test]
    fn transformed_moves_positions_and_keeps_faces_outward() {
        let moved = cube().transformed(&Transform::y_up_to_z_up());
        assert_eq!(moved.triangles, cube().triangles);
        assert_eq!(moved.colors, cube().colors);
        assert_eq!(moved.bounds(), Some(Bounds { min: [-1.0, -3.0, -2.0], max: [1.0, 3.0, 2.0] }));

        let mirror = Transform { m: [[-1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], t: [0.0; 3] };
        let mirrored = cube().transformed(&mirror);
        assert!(outward(&mirrored), "a mirrored box must still have its faces pointing out");
    }

    /// Whether every triangle's normal points away from the middle of the mesh.
    fn outward(mesh: &Mesh) -> bool {
        let center = mesh.bounds().unwrap().center();
        (0..mesh.triangles.len()).all(|at| {
            let [a, b, c] = mesh.corners(at).unwrap();
            dot(normalize(cross(sub(b, a), sub(c, a))), sub(a, center)) > 0.0
        })
    }

    #[test]
    fn cuboid_has_eight_corners_twelve_outward_triangles() {
        let mesh = cube();
        assert_eq!((mesh.positions.len(), mesh.triangles.len(), mesh.colors.len()), (8, 12, 12));
        let bounds = mesh.bounds().unwrap();
        assert_eq!(bounds, Bounds { min: [-1.0, -2.0, -3.0], max: [1.0, 2.0, 3.0] });
        assert_eq!((bounds.size(), bounds.center()), ([2.0, 4.0, 6.0], [0.0; 3]));
        assert!(outward(&mesh));
        // Two triangles on each of the six faces, with unit normals along the axes.
        let mut faces = [0; 6];
        for at in 0..12 {
            let [a, b, c] = mesh.corners(at).unwrap();
            let normal = normalize(cross(sub(b, a), sub(c, a)));
            let axis = (0..3).find(|&k| normal[k].abs() == 1.0).expect("a face normal lies along an axis");
            faces[axis * 2 + usize::from(normal[axis] > 0.0)] += 1;
        }
        assert_eq!(faces, [2; 6]);
        assert!(mesh.colors.iter().all(|&color| color == [200, 100, 50]));
        assert_eq!(mesh.validate(), Ok(()));
    }

    #[test]
    fn quads_triangles_and_append_keep_indices_right() {
        let mut mesh = Mesh::default();
        assert_eq!(mesh.bounds(), None);
        mesh.push_triangle([0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1, 1, 1]);
        mesh.push_quad([[0.0; 3], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]], [2, 2, 2]);
        assert_eq!(mesh.triangles, [[0, 1, 2], [3, 4, 5], [3, 5, 6]]);
        let mut joined = cube();
        joined.append(&mesh);
        assert_eq!(joined.triangles[12..], [[8, 9, 10], [11, 12, 13], [11, 13, 14]]);
        assert_eq!(joined.colors[12..], [[1, 1, 1], [2, 2, 2], [2, 2, 2]]);
        assert_eq!(joined.positions.len(), 15);
        assert_eq!(joined.validate(), Ok(()));
    }

    fn soup(rng: &mut Rng, triangles: usize) -> Mesh {
        let mut mesh = Mesh::default();
        for _ in 0..triangles {
            let center = [rng.range(-5.0, 5.0), rng.range(-5.0, 5.0), rng.range(0.0, 6.0)];
            let corner = |rng: &mut Rng| [0, 1, 2].map(|k| center[k] + rng.range(-0.6, 0.6));
            let (a, b, c) = (corner(rng), corner(rng), corner(rng));
            mesh.push_triangle(a, b, c, [128; 3]);
        }
        mesh
    }

    fn brute_force(mesh: &Mesh, origin: [f32; 3], direction: [f32; 3]) -> Option<(f32, u32)> {
        let d = normalize(direction);
        let hits = (0..mesh.triangles.len()).filter_map(|at| Some((ray_triangle(origin, d, &mesh.corners(at)?)?, at as u32)));
        hits.min_by(|a, b| a.0.total_cmp(&b.0))
    }

    #[test]
    fn picker_agrees_with_testing_every_triangle() {
        let mut rng = Rng(42);
        let mesh = soup(&mut rng, 4000);
        let picker = Picker::new(&mesh);
        assert!(picker.dims.iter().all(|&d| d > 1), "the grid should have more than one cell: {:?}", picker.dims);
        let (mut hits, mut misses) = (0, 0);
        for round in 0..1500 {
            let inside = [rng.range(-5.0, 5.0), rng.range(-5.0, 5.0), rng.range(0.0, 6.0)];
            let (origin, direction) = match round % 3 {
                // From inside the model, any direction (not unit length).
                0 => (inside, [rng.range(-3.0, 3.0), rng.range(-3.0, 3.0), rng.range(-3.0, 3.0)]),
                // From outside, aimed into the model.
                1 => {
                    let origin = [rng.range(-40.0, 40.0), rng.range(-40.0, 40.0), rng.range(8.0, 30.0)];
                    (origin, sub(inside, origin))
                }
                // From outside, along an axis: the ray runs exactly parallel to two sets of cell walls.
                _ => ([inside[0], inside[1], -20.0], [0.0, 0.0, 2.0]),
            };
            let expected = brute_force(&mesh, origin, direction);
            let found = picker.hit(origin, direction);
            match (expected, found) {
                (Some((distance, triangle)), Some(hit)) => {
                    assert!(
                        hit.triangle == triangle || (hit.distance - distance).abs() < 1e-3,
                        "round {round}: {hit:?} vs {distance} {triangle}"
                    );
                    assert!(close(hit.point, add(origin, scale(normalize(direction), hit.distance))));
                    hits += 1;
                }
                (None, None) => misses += 1,
                _ => panic!("round {round}: expected {expected:?}, found {found:?}"),
            }
        }
        assert!(hits > 500 && misses > 0, "{hits} hits, {misses} misses");
    }

    #[test]
    fn picker_hits_a_box_from_outside_and_inside() {
        let mut mesh = Mesh::default();
        mesh.push_cuboid([-1.0, 4.0, 0.0], [1.0, 6.0, 2.0], [9, 9, 9]);
        let picker = Picker::new(&mesh);

        let hit = picker.hit([0.2, 0.0, 1.0], [0.0, 10.0, 0.0]).unwrap();
        assert!((hit.distance - 4.0).abs() < 1e-5 && close(hit.point, [0.2, 4.0, 1.0]), "{hit:?}");
        let [a, b, c] = mesh.corners(hit.triangle as usize).unwrap();
        assert!([a, b, c].iter().all(|p| p[1] == 4.0), "the near face is the one at y = 4");

        let from_inside = picker.hit([0.0, 5.0, 1.0], [0.0, 0.0, 1.0]).unwrap();
        assert!((from_inside.distance - 1.0).abs() < 1e-5, "{from_inside:?}");
        let from_behind = picker.hit([0.0, 9.0, 1.0], [0.0, -1.0, 0.0]).unwrap();
        assert!((from_behind.distance - 3.0).abs() < 1e-5, "{from_behind:?}");

        assert_eq!(picker.hit([0.2, 0.0, 1.0], [0.0, -1.0, 0.0]), None, "pointing away");
        assert_eq!(picker.hit([5.0, 0.0, 1.0], [0.0, 1.0, 0.0]), None, "passing beside");
        assert_eq!(picker.hit([0.0, 0.0, 1.0], [0.0; 3]), None, "no direction");
        assert_eq!(picker.hit([f32::NAN, 0.0, 1.0], [0.0, 1.0, 0.0]), None);
    }

    #[test]
    fn picker_copes_with_flat_empty_and_damaged_meshes() {
        let empty = Picker::new(&Mesh::default());
        assert_eq!(empty.hit([0.0; 3], [0.0, 1.0, 0.0]), None);

        let mut floor = Mesh::default();
        floor.push_quad([[-5.0, -5.0, 0.0], [5.0, -5.0, 0.0], [5.0, 5.0, 0.0], [-5.0, 5.0, 0.0]], [1, 2, 3]);
        let hit = Picker::new(&floor).hit([1.0, 2.0, 3.0], [0.0, 0.0, -1.0]).unwrap();
        assert!((hit.distance - 3.0).abs() < 1e-5 && close(hit.point, [1.0, 2.0, 0.0]), "{hit:?}");

        // A triangle that cannot be hit is passed over, and the others keep their numbers.
        let mut damaged = floor.clone();
        damaged.triangles.insert(0, [0, 1, 77]);
        damaged.colors.insert(0, [0; 3]);
        damaged.positions.push([f32::NAN; 3]);
        let hit = Picker::new(&damaged).hit([-1.0, -2.0, 3.0], [0.0, 0.0, -1.0]).unwrap();
        assert!(hit.triangle == 1 || hit.triangle == 2, "{hit:?}");

        // A few huge triangles in a grid sized for many small ones must not list themselves in every cell.
        let mut rng = Rng(7);
        let mut mixed = soup(&mut rng, 3000);
        for _ in 0..200 {
            mixed.push_triangle(
                [-5.0, -5.0, rng.range(0.0, 6.0)],
                [5.0, -5.0, rng.range(0.0, 6.0)],
                [0.0, 5.0, rng.range(0.0, 6.0)],
                [0; 3],
            );
        }
        let picker = Picker::new(&mixed);
        assert!(picker.items.len() <= 8 * mixed.triangles.len() + 1024, "{} entries", picker.items.len());
        for _ in 0..200 {
            let origin = [rng.range(-5.0, 5.0), rng.range(-5.0, 5.0), 10.0];
            let direction = [rng.range(-0.3, 0.3), rng.range(-0.3, 0.3), -1.0];
            assert_eq!(
                picker.hit(origin, direction).map(|hit| hit.triangle),
                brute_force(&mixed, origin, direction).map(|hit| hit.1)
            );
        }
    }

    #[test]
    fn ray_triangle_measures_distance_from_either_side() {
        let triangle = [[0.0, 5.0, 0.0], [4.0, 5.0, 0.0], [0.0, 5.0, 4.0]];
        assert_eq!(ray_triangle([1.0, 0.0, 1.0], [0.0, 1.0, 0.0], &triangle), Some(5.0));
        assert_eq!(ray_triangle([1.0, 9.0, 1.0], [0.0, -1.0, 0.0], &triangle), Some(4.0));
        assert_eq!(ray_triangle([3.0, 0.0, 3.0], [0.0, 1.0, 0.0], &triangle), None, "beyond the slanted edge");
        assert_eq!(ray_triangle([1.0, 6.0, 1.0], [0.0, 1.0, 0.0], &triangle), None, "behind the ray");
        assert_eq!(ray_triangle([1.0, 0.0, 1.0], [1.0, 0.0, 0.0], &triangle), None, "parallel");
    }
}
