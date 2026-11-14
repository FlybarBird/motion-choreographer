//! Wavefront OBJ with its MTL colours.

use crate::mesh::{Mesh, DEFAULT_COLOR};
use crate::{too_many_triangles, unit_to_byte, MAX_TRIANGLES};
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

/// `sibling` is asked for each `mtllib` file name.
pub fn parse(bytes: &[u8], sibling: &dyn Fn(&str) -> Option<Vec<u8>>) -> Result<Mesh, String> {
    let mut reader = Reader {
        mesh: Mesh::default(),
        vertex_colors: Vec::new(),
        materials: HashMap::new(),
        material: None,
        corners: Vec::new(),
        sibling,
    };
    // A line ending in a backslash carries on with the next one; errors name the line where it began.
    let mut joined = String::new();
    let mut joined_from = 0;
    for (at, raw) in lines(bytes).enumerate() {
        let text = String::from_utf8_lossy(raw);
        let text = without_comment(&text);
        if let Some(head) = text.strip_suffix('\\') {
            if joined.is_empty() {
                joined_from = at + 1;
            }
            joined.push_str(head);
            joined.push(' ');
        } else if joined.is_empty() {
            reader.line(text, at + 1)?;
        } else {
            joined.push_str(text);
            reader.line(&joined, joined_from)?;
            joined.clear();
        }
    }
    if !joined.is_empty() {
        reader.line(&joined, joined_from)?;
    }
    if reader.mesh.triangles.is_empty() {
        return Err("The file has no faces to draw".into());
    }
    Ok(reader.mesh)
}

/// Material name -> its diffuse colour (`Kd`) as bytes.
pub fn parse_mtl(bytes: &[u8]) -> HashMap<String, [u8; 3]> {
    let mut colors = HashMap::new();
    let mut name = None;
    for raw in lines(bytes) {
        let text = String::from_utf8_lossy(raw);
        let (keyword, rest) = split_keyword(without_comment(&text));
        match keyword {
            "newmtl" => name = Some(rest.to_string()),
            "Kd" | "kd" => {
                let mut values = rest.split_ascii_whitespace().map_while(|word| word.parse::<f32>().ok());
                // A single number stands for all three channels; `Kd spectral ...` and `Kd xyz ...` are passed over.
                let rgb = match (values.next(), values.next(), values.next()) {
                    (Some(r), Some(g), Some(b)) => [r, g, b],
                    (Some(v), None, _) => [v; 3],
                    _ => continue,
                };
                if let Some(name) = &name {
                    colors.insert(name.clone(), rgb.map(unit_to_byte));
                }
            }
            _ => {}
        }
    }
    colors
}

/// Writes (obj_text, mtl_text) for a mesh, with a `usemtl` wherever the colour changes from one face to
/// the next. `mtl_name` is the file name put in `mtllib`. For tests and sample files.
pub fn write(mesh: &Mesh, mtl_name: &str) -> (String, String) {
    let mut obj = format!("mtllib {mtl_name}\n");
    let mut mtl = String::new();
    for [x, y, z] in &mesh.positions {
        let _ = writeln!(obj, "v {x} {y} {z}");
    }
    let mut written = HashSet::new();
    let mut current = None;
    for (at, [a, b, c]) in mesh.triangles.iter().enumerate() {
        let color = mesh.colors.get(at).copied().unwrap_or(DEFAULT_COLOR);
        if current != Some(color) {
            current = Some(color);
            let name = format!("color_{:02x}{:02x}{:02x}", color[0], color[1], color[2]);
            if written.insert(color) {
                let [r, g, b] = color.map(|v| v as f32 / 255.0);
                let _ = writeln!(mtl, "newmtl {name}\nKd {r:.6} {g:.6} {b:.6}\n");
            }
            let _ = writeln!(obj, "usemtl {name}");
        }
        let _ = writeln!(obj, "f {} {} {}", *a as u64 + 1, *b as u64 + 1, *c as u64 + 1);
    }
    (obj, mtl)
}

struct Reader<'a> {
    mesh: Mesh,
    /// Filled only up to the last vertex that carried a colour.
    vertex_colors: Vec<Option<[u8; 3]>>,
    materials: HashMap<String, [u8; 3]>,
    /// Colour of the material in force, when the MTL files know it.
    material: Option<[u8; 3]>,
    corners: Vec<u32>,
    sibling: &'a dyn Fn(&str) -> Option<Vec<u8>>,
}

impl Reader<'_> {
    fn line(&mut self, text: &str, number: usize) -> Result<(), String> {
        let (keyword, rest) = split_keyword(text);
        match keyword {
            "v" => self.vertex(rest, number)?,
            "f" => self.face(rest, number)?,
            "usemtl" => self.material = self.materials.get(rest).copied(),
            "mtllib" => self.load_materials(rest),
            _ => {}
        }
        Ok(())
    }

    /// `v x y z`, optionally followed by `w` (not needed for drawing) or by a colour `r g b`.
    fn vertex(&mut self, rest: &str, number: usize) -> Result<(), String> {
        let mut words = rest.split_ascii_whitespace();
        let mut position = [0.0f32; 3];
        for (given, value) in position.iter_mut().enumerate() {
            let word =
                words.next().ok_or_else(|| format!("line {number}: a vertex needs three numbers (x y z), but has {given}"))?;
            *value = word.parse().map_err(|_| format!("line {number}: a vertex position is not a number (\"{word}\")"))?;
            if !value.is_finite() {
                return Err(format!("line {number}: a vertex position is too large to use (\"{word}\")"));
            }
        }
        self.mesh.positions.push(position);
        let mut extra = words.map_while(|word| word.parse::<f32>().ok());
        if let (Some(r), Some(g), Some(b)) = (extra.next(), extra.next(), extra.next()) {
            self.vertex_colors.resize(self.mesh.positions.len() - 1, None);
            self.vertex_colors.push(Some([r, g, b].map(unit_to_byte)));
        }
        Ok(())
    }

    /// `f` followed by corners written `v`, `v/vt`, `v/vt/vn` or `v//vn`; only `v` matters here.
    fn face(&mut self, rest: &str, number: usize) -> Result<(), String> {
        let defined = self.mesh.positions.len();
        self.corners.clear();
        for word in rest.split_ascii_whitespace() {
            let index: i64 = word
                .split('/')
                .next()
                .and_then(|index| index.parse().ok())
                .ok_or_else(|| format!("line {number}: a face has a vertex number that cannot be read (\"{word}\")"))?;
            // Positive numbers count from the start of the file, negative ones back from the latest vertex.
            let position = if index > 0 { index - 1 } else { (defined as i64).saturating_add(index) };
            if index == 0 || position < 0 || position >= defined as i64 {
                return Err(format!("line {number}: a face refers to vertex {index}, but the file has {defined} so far"));
            }
            self.corners.push(position as u32);
        }
        for second in 1..self.corners.len().saturating_sub(1) {
            if self.mesh.triangles.len() >= MAX_TRIANGLES {
                return Err(too_many_triangles());
            }
            let triangle = [self.corners[0], self.corners[second], self.corners[second + 1]];
            let color = self.material.or_else(|| self.vertex_color(triangle)).unwrap_or(DEFAULT_COLOR);
            self.mesh.triangles.push(triangle);
            self.mesh.colors.push(color);
        }
        Ok(())
    }

    /// The average of the corners' own colours, when all three have one.
    fn vertex_color(&self, triangle: [u32; 3]) -> Option<[u8; 3]> {
        let mut sum = [0u32; 3];
        for index in triangle {
            let color = self.vertex_colors.get(index as usize).copied().flatten()?;
            for k in 0..3 {
                sum[k] += color[k] as u32;
            }
        }
        Some(sum.map(|v| ((v + 1) / 3) as u8))
    }

    fn load_materials(&mut self, rest: &str) {
        let sibling = self.sibling;
        // A file name may hold spaces, so the whole line is tried as one name before it is taken as a list.
        // Exporters also write full paths; the file itself still sits beside the model.
        let file_name = rest.rsplit(['/', '\\']).next().unwrap_or(rest);
        match sibling(rest).or_else(|| sibling(file_name)) {
            Some(bytes) => self.materials.extend(parse_mtl(&bytes)),
            None => {
                for name in rest.split_ascii_whitespace() {
                    if let Some(bytes) = sibling(name) {
                        self.materials.extend(parse_mtl(&bytes));
                    }
                }
            }
        }
    }
}

/// Lines ended by LF, CR LF or a lone CR.
fn lines(bytes: &[u8]) -> impl Iterator<Item = &[u8]> {
    let mut rest = bytes;
    std::iter::from_fn(move || {
        if rest.is_empty() {
            return None;
        }
        let end = rest.iter().position(|&b| b == b'\n' || b == b'\r').unwrap_or(rest.len());
        let (line, after) = rest.split_at(end);
        rest = after.strip_prefix(b"\r\n").or_else(|| after.get(1..)).unwrap_or(&[]);
        Some(line)
    })
}

fn without_comment(line: &str) -> &str {
    line.split('#').next().unwrap_or("").trim()
}

fn split_keyword(line: &str) -> (&str, &str) {
    let (keyword, rest) = line.split_once(|c: char| c.is_ascii_whitespace()).unwrap_or((line, ""));
    (keyword, rest.trim_start())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{damage, Rng};

    const CUBE_VERTICES: &str = "v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nv 0 0 1\nv 1 0 1\nv 1 1 1\nv 0 1 1\n";
    const CUBE_QUADS: &str = "f 1 4 3 2\nf 5 6 7 8\nf 1 2 6 5\nf 3 4 8 7\nf 1 5 8 4\nf 2 3 7 6\n";

    fn no_files(_: &str) -> Option<Vec<u8>> {
        None
    }

    fn read(text: &str) -> Mesh {
        parse(text.as_bytes(), &no_files).unwrap()
    }

    /// The triangles as corner positions, which is what must survive whatever way the file numbers them.
    fn corners(mesh: &Mesh) -> Vec<[[f32; 3]; 3]> {
        (0..mesh.triangles.len()).map(|at| mesh.corners(at).unwrap()).collect()
    }

    fn cube() -> Mesh {
        read(&format!("{CUBE_VERTICES}{CUBE_QUADS}"))
    }

    #[test]
    fn cube_as_quads_is_fanned_into_twelve_triangles() {
        let mesh = cube();
        assert_eq!((mesh.positions.len(), mesh.triangles.len()), (8, 12));
        assert_eq!(mesh.triangles[..4], [[0, 3, 2], [0, 2, 1], [4, 5, 6], [4, 6, 7]]);
        assert_eq!(mesh.colors, vec![[170, 170, 170]; 12]);
        assert_eq!(mesh.validate(), Ok(()));
    }

    #[test]
    fn cube_as_triangles_matches_the_quads() {
        let triangles =
            "f 1 4 3\nf 1 3 2\nf 5 6 7\nf 5 7 8\nf 1 2 6\nf 1 6 5\nf 3 4 8\nf 3 8 7\nf 1 5 8\nf 1 8 4\nf 2 3 7\nf 2 7 6\n";
        assert_eq!(read(&format!("{CUBE_VERTICES}{triangles}")), cube());
    }

    #[test]
    fn negative_indices_count_back_from_the_latest_vertex() {
        let relative = CUBE_QUADS.replace("f 1 4 3 2", "f -8 -5 -6 -7").replace("f 2 3 7 6", "f -7 -6 -2 -3");
        assert_eq!(read(&format!("{CUBE_VERTICES}{relative}")), cube());
        // Relative to the vertices defined so far, not to the whole file.
        let mesh = read("v 0 0 0\nv 1 0 0\nv 0 1 0\nf -3 -2 -1\nv 5 5 5\nv 6 5 5\nv 5 6 5\nf -3 -2 -1\n");
        assert_eq!(mesh.triangles, [[0, 1, 2], [3, 4, 5]]);
    }

    #[test]
    fn texture_and_normal_references_are_passed_over() {
        let extras = "vt 0 0\nvt 1 0\nvn 0 0 1\nvp 0.5\n";
        let slashed = CUBE_QUADS
            .replace("f 1 4 3 2", "f 1/1/1 4/2/1 3/1/1 2/2/1")
            .replace("f 5 6 7 8", "f 5//1 6//1 7//1 8//1")
            .replace("f 1 2 6 5", "f 1/1 2/2 6/1 5/2");
        assert_eq!(read(&format!("{CUBE_VERTICES}{extras}{slashed}")), cube());
        assert_eq!(read(&format!("{CUBE_VERTICES}{}", CUBE_QUADS.replace("f 1 4 3 2", "f -8/-1/-1 -5//-1 -6/2 -7"))), cube());
    }

    #[test]
    fn line_ends_comments_blank_lines_and_continuations() {
        let text =
            format!("# a cube\r\n\r\no Cube\r\ng side\r\ns off\r\n{CUBE_VERTICES}l 1 2\r\np 1\r\nwhatever 1 2 3\r\n{CUBE_QUADS}")
                .replace('\n', "\r\n")
                .replace("\r\r\n", "\r\n")
                .replace("f 1 4 3 2", "f 1 4 \\\r\n  3 \\\r\n 2 # the bottom")
                .replace("v 1 1 1", "  v\t1 1 1   # a corner");
        assert_eq!(read(&text), cube());
        assert_eq!(read(&text.replace("\r\n", "\r")), cube(), "lone carriage returns");
        assert_eq!(read(&format!("{CUBE_VERTICES}{}", CUBE_QUADS.trim_end())), cube(), "no final line end");
        let mut bytes = b"# caf\xe9 \xff\xfe\n".to_vec();
        bytes.extend(format!("{CUBE_VERTICES}g \u{fffd}\n{CUBE_QUADS}").bytes());
        assert_eq!(parse(&bytes, &no_files).unwrap(), cube(), "text that is not UTF-8");
    }

    #[test]
    fn short_faces_are_skipped_and_big_ones_fanned() {
        let mesh = read("v 0 0 0\nv 1 0 0\nv 2 1 0\nv 1 2 0\nv 0 1 0\nf 1 2\nf 3\nf\nf 1 2 3 4 5\n");
        assert_eq!(mesh.triangles, [[0, 1, 2], [0, 2, 3], [0, 3, 4]]);
        assert_eq!(read("v 0 0 0 1.0\nv 1 0 0 1.0\nv 0 1 0 1.0\nf 1 2 3\n").positions[1], [1.0, 0.0, 0.0], "an optional w");
    }

    #[test]
    fn materials_colour_the_faces_that_use_them() {
        let mtl = "# materials\nnewmtl wood\nNs 10\nKa 1 1 1\nKd 0.6 0.4 0.2\n\nnewmtl bright one\nKd 2.0 1.0 -1\nnewmtl grey\nKd 0.5\nnewmtl odd\nKd spectral file.rfl\n";
        let obj = format!(
            "mtllib stage.mtl\n{CUBE_VERTICES}usemtl wood\nf 1 4 3 2\nusemtl bright one\nf 5 6 7 8\nusemtl grey\nf 1 2 6\n\
             usemtl missing\nf 3 4 8\nusemtl odd\nf 1 5 8\nusemtl wood\nf 2 3 7\n"
        );
        let sibling = |name: &str| (name == "stage.mtl").then(|| mtl.as_bytes().to_vec());
        let mesh = parse(obj.as_bytes(), &sibling).unwrap();
        let (wood, grey) = ([153, 102, 51], [170, 170, 170]);
        assert_eq!(mesh.colors, [wood, wood, [255, 255, 0], [255, 255, 0], [128, 128, 128], grey, grey, wood]);

        let colors = parse_mtl(mtl.as_bytes());
        assert_eq!(colors.len(), 3);
        assert_eq!(colors["wood"], wood);
        assert_eq!(parse_mtl(b"newmtl a\r\nKd 1 0 0\r\nnewmtl b\r\nKd 0 0 1\r\n")["b"], [0, 0, 255]);
    }

    #[test]
    fn material_files_are_found_by_name() {
        let obj =
            |mtllib: &str| format!("mtllib {mtllib}\nv 0 0 0\nv 1 0 0\nv 0 1 0\nusemtl red\nf 1 2 3\nusemtl blue\nf 1 2 3\n");
        let files = |name: &str| match name {
            "my stage.mtl" | "a.mtl" => Some(b"newmtl red\nKd 1 0 0\n".to_vec()),
            "b.mtl" => Some(b"newmtl blue\nKd 0 0 1\n".to_vec()),
            _ => None,
        };
        let grey = [170, 170, 170];
        let colors = |mtllib: &str| parse(obj(mtllib).as_bytes(), &files).unwrap().colors;
        assert_eq!(colors("my stage.mtl"), [[255, 0, 0], grey], "a name with a space");
        assert_eq!(colors("a.mtl b.mtl"), [[255, 0, 0], [0, 0, 255]], "several files");
        assert_eq!(colors("C:\\Users\\me\\models\\a.mtl"), [[255, 0, 0], grey], "a full path");
        assert_eq!(colors("./b.mtl"), [grey, [0, 0, 255]]);
        assert_eq!(colors("lost.mtl"), [grey, grey], "a missing file leaves the faces grey");
    }

    #[test]
    fn vertex_colours_are_averaged_over_a_face_without_material() {
        let text = "v 0 0 0 1 0 0\nv 1 0 0 0 1 0\nv 0 1 0 0 0 1\nv 1 1 0\nf 1 2 3\nf 2 3 4\nusemtl nothing\nf 1 2 3\n";
        let mesh = read(text);
        assert_eq!(mesh.colors, [[85, 85, 85], [170, 170, 170], [85, 85, 85]]);
        let sibling = |_: &str| Some(b"newmtl paint\nKd 0 0 1\n".to_vec());
        let painted = parse(format!("mtllib m.mtl\nusemtl paint\n{text}").as_bytes(), &sibling).unwrap();
        assert_eq!(painted.colors[0], [0, 0, 255], "a material wins over vertex colours");
        assert_eq!(read("v 0 0 0 1 1 1\nv 1 0 0 1 0.5 0\nv 0 1 0 1 0 0\nf 1 2 3\n").colors, [[255, 128, 85]]);
    }

    #[test]
    fn mistakes_name_their_line() {
        let fails = |text: &str| parse(text.as_bytes(), &no_files).unwrap_err();
        assert_eq!(
            fails("v 0 0 0\nv 1 0 0\nv 0 1 0\n# note\nf 1 2 4\n"),
            "line 5: a face refers to vertex 4, but the file has 3 so far"
        );
        assert_eq!(fails("v 0 0 0\nv 1 0 0\nf 1 2 3\nv 0 1 0\n"), "line 3: a face refers to vertex 3, but the file has 2 so far");
        assert_eq!(
            fails("v 0 0 0\r\nv 1 0 0\r\nv 0 1 0\r\nf 1 2 -4\r\n"),
            "line 4: a face refers to vertex -4, but the file has 3 so far"
        );
        assert!(fails("v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 0\n").starts_with("line 4: a face refers to vertex 0,"));
        assert!(fails("v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 x\n")
            .starts_with("line 4: a face has a vertex number that cannot be read (\"x\")"));
        assert!(fails("v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 99999999999999999999\n").starts_with("line 4:"));
        assert!(fails("v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 -9223372036854775808\n")
            .starts_with("line 4: a face refers to vertex -9223372036854775808"));
        assert_eq!(fails("v 0 0\nf 1 1 1\n"), "line 1: a vertex needs three numbers (x y z), but has 2");
        assert!(fails("\n\nv 0 zero 0\n").starts_with("line 3: a vertex position is not a number (\"zero\")"));
        assert!(fails("v 0 1e60 0\n").starts_with("line 1: a vertex position is too large to use"));
        assert!(fails("v 0 nan 0\n").starts_with("line 1:"));
        // A continued line is named by where it began.
        assert!(
            fails("v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 \\\n 3 \\\n 9\nf 1 2 3\n").starts_with("line 4: a face refers to vertex 9,")
        );
    }

    #[test]
    fn files_without_faces_are_refused() {
        for text in ["", "\n\n", "# nothing here\n", CUBE_VERTICES, "v 0 0 0\nv 1 0 0\nf 1 2\n", "\\", "f"] {
            assert_eq!(parse(text.as_bytes(), &no_files).unwrap_err(), "The file has no faces to draw", "{text:?}");
        }
    }

    #[test]
    fn too_many_triangles_are_refused_with_advice() {
        let mut text = String::from("v 0 0 0\nv 1 0 0\nv 0 1 0\n");
        text.push_str(&"f 1 2 3\n".repeat(MAX_TRIANGLES));
        assert_eq!(read(&text).triangles.len(), MAX_TRIANGLES);
        text.push_str("f 1 2 3\n");
        let refused = parse(text.as_bytes(), &no_files).unwrap_err();
        assert!(refused.contains("more than 1.5 million triangles") && refused.contains("lighter"), "{refused}");
    }

    #[test]
    fn written_files_read_back_the_same() {
        let mut mesh = Mesh::default();
        mesh.push_cuboid([-1.5, 0.25, 0.1], [2.0, 3.0, 1.0e-3], [10, 20, 30]);
        mesh.push_cuboid([5.0, 5.0, 5.0], [6.0, 7.0, 8.0], [200, 0, 255]);
        mesh.push_triangle([0.1, 0.2, 0.3], [1.0 / 3.0, -2.0e9, 7.0e-12], [0.0, 1.0, 2.0], [10, 20, 30]);
        mesh.push_quad([[0.0; 3], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]], [170, 170, 170]);
        let (obj, mtl) = write(&mesh, "my room.mtl");
        assert_eq!(obj.matches("usemtl").count(), 4, "one group for each run of a colour");
        assert_eq!(mtl.matches("newmtl").count(), 3, "one material for each colour");
        let sibling = |name: &str| (name == "my room.mtl").then(|| mtl.clone().into_bytes());
        assert_eq!(parse(obj.as_bytes(), &sibling).unwrap(), mesh);
        assert_eq!(parse(obj.as_bytes(), &no_files).unwrap().colors, vec![[170, 170, 170]; mesh.triangles.len()]);
        assert_eq!(corners(&parse(obj.as_bytes(), &sibling).unwrap()), corners(&mesh));
    }

    #[test]
    fn damaged_obj_files_never_panic() {
        let mut mesh = Mesh::default();
        mesh.push_cuboid([0.0; 3], [1.0, 2.0, 3.0], [10, 20, 30]);
        mesh.push_cuboid([5.0; 3], [6.0; 3], [200, 0, 255]);
        let (obj, mtl) = write(&mesh, "room.mtl");
        let obj = format!("# sample\n{obj}v 1 2 3 0.5 0.5 0.5\nf -1/1/1 -2//3 \\\n -3/2\nvt 0 0\n");
        let mut rng = Rng(0xDEAD_BEEF_CAFE_F00D);
        let mut read = 0;
        for round in 0..4000 {
            let bytes = damage(obj.as_bytes(), &mut rng);
            let materials = if round % 2 == 0 { mtl.clone().into_bytes() } else { damage(mtl.as_bytes(), &mut rng) };
            if let Ok(mesh) = parse(&bytes, &|_| Some(materials.clone())) {
                assert_eq!(mesh.validate(), Ok(()));
                read += 1;
            }
            parse_mtl(&bytes);
        }
        assert!(read > 0, "some damaged files should still be readable");
    }
}
