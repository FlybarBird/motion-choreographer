//! Stage models: reading them, storing them, drawing them. No dependencies, so the same
//! pixels come out on every machine.

mod math;
pub mod mesh;
pub mod obj;
pub mod render;
pub mod tds;

pub type Vec3 = [f32; 3];

/// Most triangles an imported model may have.
pub const MAX_TRIANGLES: usize = 1_500_000;

/// Reads a model by its file name's extension (.obj or .3ds, case-insensitive).
/// `sibling(name)` returns the bytes of a file that sits beside the model (used for the OBJ's .mtl), or None.
pub fn import(name: &str, bytes: &[u8], sibling: &dyn Fn(&str) -> Option<Vec<u8>>) -> Result<mesh::Mesh, String> {
    let extension = name.rsplit_once('.').map(|(_, extension)| extension.to_ascii_lowercase());
    match extension.as_deref() {
        Some("obj") => obj::parse(bytes, sibling),
        Some("3ds") => tds::parse(bytes),
        _ => {
            Err(format!("\"{name}\" is not a kind of model this program can read. Use a Wavefront .obj or an Autodesk .3ds file"))
        }
    }
}

pub(crate) fn too_many_triangles() -> String {
    format!(
        "The model has more than {} million triangles, which is the most this program can show. \
         Export a lighter version of the model (less detail, or without the small fittings) and try again",
        MAX_TRIANGLES as f64 / 1e6
    )
}

/// A colour channel given as 0..1 in a file, as a byte. Files are taken to hold display colours, so there is no gamma step.
pub(crate) fn unit_to_byte(value: f32) -> u8 {
    (value * 255.0).round().clamp(0.0, 255.0) as u8
}

#[cfg(test)]
pub(crate) mod testing {
    /// Xorshift: the same damaged files on every run, so a failure can be repeated.
    pub struct Rng(pub u64);

    impl Rng {
        pub fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        pub fn below(&mut self, limit: usize) -> usize {
            (self.next() % limit.max(1) as u64) as usize
        }

        pub fn unit(&mut self) -> f32 {
            (self.next() >> 40) as f32 / (1u64 << 24) as f32
        }

        pub fn range(&mut self, low: f32, high: f32) -> f32 {
            low + (high - low) * self.unit()
        }
    }

    /// One damaged copy of `valid`: cut short, a few bytes changed, or a run of bytes replaced.
    pub fn damage(valid: &[u8], rng: &mut Rng) -> Vec<u8> {
        let mut bytes = valid.to_vec();
        match rng.below(4) {
            0 => bytes.truncate(rng.below(valid.len() + 1)),
            1 => {
                for _ in 0..1 + rng.below(4) {
                    let at = rng.below(bytes.len());
                    bytes[at] ^= 1 << rng.below(8);
                }
            }
            2 => {
                for _ in 0..1 + rng.below(8) {
                    let at = rng.below(bytes.len());
                    bytes[at] = rng.next() as u8;
                }
            }
            _ => {
                let from = rng.below(bytes.len());
                let to = (from + rng.below(16)).min(bytes.len());
                let fill = [0x00, 0xFF, b'\n', b'-', b'/', b'9', b'\\', b' '][rng.below(8)];
                bytes[from..to].fill(fill);
            }
        }
        bytes
    }
}

#[cfg(test)]
mod tests {
    use crate::mesh::Mesh;
    use crate::{import, obj, tds};

    #[test]
    fn import_goes_by_the_file_extension() {
        let mut cube = Mesh::default();
        cube.push_cuboid([0.0; 3], [1.0; 3], [10, 20, 30]);
        let (obj, mtl) = obj::write(&cube, "cube.mtl");
        let sibling = |name: &str| (name == "cube.mtl").then(|| mtl.clone().into_bytes());
        assert_eq!(import("Hall/Cube.OBJ", obj.as_bytes(), &sibling).unwrap().colors[0], [10, 20, 30]);
        assert_eq!(import("cube.3ds", &tds::write(&cube, "Cube"), &sibling).unwrap().triangles.len(), 12);
        let refused = import("cube.fbx", b"", &sibling).unwrap_err();
        assert!(refused.contains("cube.fbx") && refused.contains(".obj") && refused.contains(".3ds"), "{refused}");
        assert!(import("cube", b"", &sibling).is_err());
    }
}
