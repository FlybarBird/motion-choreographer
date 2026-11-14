//! Autodesk 3DS.
//!
//! The file is a tree of sections ("chunks"), all little-endian. Each starts with `u16 id, u32 length`,
//! where the length counts the 6-byte heading too, then holds its own data followed by its sub-sections.

use crate::mesh::{determinant, Mesh, DEFAULT_COLOR};
use crate::{too_many_triangles, unit_to_byte, MAX_TRIANGLES};
use std::collections::{BTreeMap, BTreeSet, HashMap};

const MAIN: u16 = 0x4D4D;
const FILE_VERSION: u16 = 0x0002;
const EDITOR: u16 = 0x3D3D;
const MESH_VERSION: u16 = 0x3D3E;
/// Read past: exporters rarely set it meaningfully, and the program lets the user pick the units.
const MASTER_SCALE: u16 = 0x0100;
const MATERIAL: u16 = 0xAFFF;
const MATERIAL_NAME: u16 = 0xA000;
const MATERIAL_DIFFUSE: u16 = 0xA020;
const COLOR_FLOATS: u16 = 0x0010;
const COLOR_BYTES: u16 = 0x0011;
const COLOR_BYTES_GAMMA: u16 = 0x0012;
const COLOR_FLOATS_GAMMA: u16 = 0x0013;
const OBJECT: u16 = 0x4000;
const TRIMESH: u16 = 0x4100;
const VERTICES: u16 = 0x4110;
const FACES: u16 = 0x4120;
const FACE_MATERIAL: u16 = 0x4130;
const LOCAL_AXES: u16 = 0x4160;

/// Both counts in a trimesh are u16.
const MAX_PER_OBJECT: usize = 65535;
/// Other programs expect object names of at most ten characters.
const MAX_NAME_LEN: usize = 10;

pub fn parse(bytes: &[u8]) -> Result<Mesh, String> {
    if u16_at(bytes, 0) != Some(MAIN) || bytes.len() < 6 {
        return Err("This is not a 3DS file".into());
    }
    let mut mesh = Mesh::default();
    let mut top = Chunks { bytes, at: 0, end: bytes.len() };
    if let Some(main) = top.next()? {
        let mut chunks = Chunks::inside(bytes, main.body, &main);
        while let Some(chunk) = chunks.next()? {
            if chunk.id == EDITOR {
                read_editor(bytes, &chunk, &mut mesh)?;
            }
        }
    }
    if mesh.triangles.is_empty() {
        return Err("The file has no faces to draw".into());
    }
    Ok(mesh)
}

/// Writes ONE object named `name` holding the whole mesh, with a material per distinct colour and the
/// lists of faces that use each. A mesh with more corners or faces than one 3DS object can hold is split
/// into several, numbered. For tests and sample files.
pub fn write(mesh: &Mesh, name: &str) -> Vec<u8> {
    let colors: Vec<[u8; 3]> =
        (0..mesh.triangles.len()).map(|at| mesh.colors.get(at).copied().unwrap_or(DEFAULT_COLOR)).collect();

    let mut editor = chunk(MESH_VERSION, &3u32.to_le_bytes());
    for color in colors.iter().collect::<BTreeSet<_>>() {
        let diffuse = chunk(MATERIAL_DIFFUSE, &chunk(COLOR_BYTES, color));
        editor.extend(chunk(MATERIAL, &[chunk(MATERIAL_NAME, &material_name(color)), diffuse].concat()));
    }
    editor.extend(chunk(MASTER_SCALE, &1f32.to_le_bytes()));

    let mut part = Part::default();
    let mut objects = 0;
    for (at, color) in colors.iter().enumerate() {
        let Some(corners) = mesh.corners(at) else { continue };
        if part.positions.len() + 3 > MAX_PER_OBJECT || part.faces.len() == MAX_PER_OBJECT {
            editor.extend(part.object(name, objects));
            objects += 1;
            part = Part::default();
        }
        let face = [0, 1, 2].map(|k| {
            *part.local.entry(mesh.triangles[at][k]).or_insert_with(|| {
                part.positions.push(corners[k]);
                (part.positions.len() - 1) as u16
            })
        });
        part.by_color.entry(*color).or_default().push(part.faces.len() as u16);
        part.faces.push(face);
    }
    if !part.faces.is_empty() || objects == 0 {
        editor.extend(part.object(name, objects));
    }

    chunk(MAIN, &[chunk(FILE_VERSION, &3u32.to_le_bytes()), chunk(EDITOR, &editor)].concat())
}

/// The corners and faces gathered for one 3DS object while writing.
#[derive(Default)]
struct Part {
    positions: Vec<[f32; 3]>,
    faces: Vec<[u16; 3]>,
    /// Mesh vertex number -> its number within this object.
    local: HashMap<u32, u16>,
    by_color: BTreeMap<[u8; 3], Vec<u16>>,
}

impl Part {
    /// The OBJECT section; objects after the first get a number at the end of their name.
    fn object(&self, name: &str, number: usize) -> Vec<u8> {
        let mut vertices = (self.positions.len() as u16).to_le_bytes().to_vec();
        vertices.extend(self.positions.iter().flatten().flat_map(|v| v.to_le_bytes()));
        // The fourth number of a face is its edge flags: all three edges visible.
        let mut faces = (self.faces.len() as u16).to_le_bytes().to_vec();
        faces.extend(self.faces.iter().flat_map(|face| [face[0], face[1], face[2], 0x0007]).flat_map(u16::to_le_bytes));
        for (color, list) in &self.by_color {
            let mut body = material_name(color);
            body.extend((list.len() as u16).to_le_bytes());
            body.extend(list.iter().flat_map(|face| face.to_le_bytes()));
            faces.extend(chunk(FACE_MATERIAL, &body));
        }
        let suffix = if number == 0 { String::new() } else { (number + 1).to_string() };
        let mut body: Vec<u8> =
            name.bytes().filter(|b| b.is_ascii() && *b != 0).take(MAX_NAME_LEN.saturating_sub(suffix.len())).collect();
        body.extend(suffix.bytes());
        body.push(0);
        body.extend(chunk(TRIMESH, &[chunk(VERTICES, &vertices), chunk(FACES, &faces)].concat()));
        chunk(OBJECT, &body)
    }
}

fn material_name(color: &[u8; 3]) -> Vec<u8> {
    format!("color_{:02x}{:02x}{:02x}\0", color[0], color[1], color[2]).into_bytes()
}

fn chunk(id: u16, body: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(6 + body.len());
    bytes.extend(id.to_le_bytes());
    bytes.extend((body.len() as u32 + 6).to_le_bytes());
    bytes.extend(body);
    bytes
}

struct Chunk {
    id: u16,
    /// Offset of the heading in the file; `body..end` is what follows the heading.
    start: usize,
    body: usize,
    end: usize,
}

/// Walks the sections that lie one after another in `at..end`.
struct Chunks<'a> {
    bytes: &'a [u8],
    at: usize,
    end: usize,
}

impl<'a> Chunks<'a> {
    /// The sub-sections of `parent`, which start at `from` (after the parent's own data).
    fn inside(bytes: &'a [u8], from: usize, parent: &Chunk) -> Chunks<'a> {
        Chunks { bytes, at: from, end: parent.end }
    }

    fn next(&mut self) -> Result<Option<Chunk>, String> {
        let start = self.at;
        if start >= self.end {
            return Ok(None);
        }
        let heading = slice(self.bytes, start, 6, self.end).ok_or_else(|| damaged(start, "a section is cut short"))?;
        let id = u16::from_le_bytes([heading[0], heading[1]]);
        let length = u32::from_le_bytes([heading[2], heading[3], heading[4], heading[5]]) as usize;
        if length < 6 {
            return Err(damaged(start, "a section is shorter than its own heading"));
        }
        // Every section is at least 6 bytes, so the walk always moves on and cannot loop.
        let end = start.checked_add(length).filter(|&end| end <= self.end).ok_or_else(|| {
            let past = if self.end == self.bytes.len() { "the end" } else { "the section that holds it" };
            damaged(start, &format!("a section runs past {past}"))
        })?;
        self.at = end;
        Ok(Some(Chunk { id, start, body: start + 6, end }))
    }
}

fn read_editor(bytes: &[u8], editor: &Chunk, mesh: &mut Mesh) -> Result<(), String> {
    // Materials first, wherever they sit, so that every object finds its colours.
    let mut materials = HashMap::new();
    let mut chunks = Chunks::inside(bytes, editor.body, editor);
    while let Some(chunk) = chunks.next()? {
        if chunk.id == MATERIAL {
            read_material(bytes, &chunk, &mut materials)?;
        }
    }
    let mut chunks = Chunks::inside(bytes, editor.body, editor);
    while let Some(chunk) = chunks.next()? {
        if chunk.id == OBJECT {
            let (name, after_name) = name_at(bytes, chunk.body, chunk.end)?;
            let mut parts = Chunks::inside(bytes, after_name, &chunk);
            while let Some(part) = parts.next()? {
                if part.id == TRIMESH {
                    read_trimesh(bytes, &part, name, &materials, mesh)?;
                }
            }
        }
    }
    Ok(())
}

fn read_material<'a>(bytes: &'a [u8], material: &Chunk, materials: &mut HashMap<&'a [u8], [u8; 3]>) -> Result<(), String> {
    let (mut name, mut color) = (None, DEFAULT_COLOR);
    let mut chunks = Chunks::inside(bytes, material.body, material);
    while let Some(chunk) = chunks.next()? {
        match chunk.id {
            MATERIAL_NAME => name = Some(name_at(bytes, chunk.body, chunk.end)?.0),
            MATERIAL_DIFFUSE => color = read_color(bytes, &chunk)?.unwrap_or(color),
            _ => {}
        }
    }
    if let Some(name) = name {
        materials.insert(name, color);
    }
    Ok(())
}

/// A colour may be given as bytes or as 0..1 numbers, plain or gamma-corrected; the plain form wins when
/// a file carries both.
fn read_color(bytes: &[u8], holder: &Chunk) -> Result<Option<[u8; 3]>, String> {
    let (mut plain, mut corrected) = (None, None);
    let mut chunks = Chunks::inside(bytes, holder.body, holder);
    while let Some(chunk) = chunks.next()? {
        let cut_short = || damaged(chunk.start, "a colour is cut short");
        let color = match chunk.id {
            COLOR_BYTES | COLOR_BYTES_GAMMA => {
                let rgb = slice(bytes, chunk.body, 3, chunk.end).ok_or_else(cut_short)?;
                [rgb[0], rgb[1], rgb[2]]
            }
            COLOR_FLOATS | COLOR_FLOATS_GAMMA => {
                let rgb = slice(bytes, chunk.body, 12, chunk.end).ok_or_else(cut_short)?;
                [0, 4, 8].map(|at| unit_to_byte(f32_in(rgb, at)))
            }
            _ => continue,
        };
        if matches!(chunk.id, COLOR_BYTES | COLOR_FLOATS) {
            plain = Some(color);
        } else {
            corrected = Some(color);
        }
    }
    Ok(plain.or(corrected))
}

fn read_trimesh(
    bytes: &[u8],
    trimesh: &Chunk,
    name: &[u8],
    materials: &HashMap<&[u8], [u8; 3]>,
    mesh: &mut Mesh,
) -> Result<(), String> {
    let mut vertices: Vec<[f32; 3]> = Vec::new();
    let mut faces: Vec<[u16; 3]> = Vec::new();
    let mut colors: Vec<[u8; 3]> = Vec::new();
    let mut mirrored = false;
    let mut faces_start = trimesh.start;
    let mut chunks = Chunks::inside(bytes, trimesh.body, trimesh);
    while let Some(chunk) = chunks.next()? {
        match chunk.id {
            VERTICES => {
                let data = counted(bytes, chunk.body, 12, chunk.end)
                    .ok_or_else(|| damaged(chunk.start, "a list of vertices is cut short"))?;
                vertices = data.as_chunks::<12>().0.iter().map(|v| [0, 4, 8].map(|at| f32_in(v, at))).collect();
                if vertices.iter().flatten().any(|v| !v.is_finite()) {
                    return Err(damaged(chunk.start, "a vertex position is not a usable number"));
                }
            }
            FACES => {
                let data = counted(bytes, chunk.body, 8, chunk.end)
                    .ok_or_else(|| damaged(chunk.start, "a list of faces is cut short"))?;
                faces =
                    data.as_chunks::<8>().0.iter().map(|f| [0, 2, 4].map(|at| u16::from_le_bytes([f[at], f[at + 1]]))).collect();
                colors = vec![DEFAULT_COLOR; faces.len()];
                faces_start = chunk.start;
                // The lists saying which faces use which material sit inside the face section, after the faces.
                let mut lists = Chunks::inside(bytes, chunk.body + 2 + data.len(), &chunk);
                while let Some(list) = lists.next()? {
                    if list.id == FACE_MATERIAL {
                        let (material, after_name) = name_at(bytes, list.body, list.end)?;
                        let data = counted(bytes, after_name, 2, list.end)
                            .ok_or_else(|| damaged(list.start, "a list of faces is cut short"))?;
                        for face in data.as_chunks::<2>().0.iter().map(|f| u16::from_le_bytes([f[0], f[1]])) {
                            let Some(color) = colors.get_mut(face as usize) else {
                                let what = format!("a material is given to face {face}, but the object has only {}", faces.len());
                                return Err(damaged(list.start, &what));
                            };
                            *color = materials.get(material).copied().unwrap_or(*color);
                        }
                    }
                }
            }
            LOCAL_AXES => {
                // Three axis vectors, then the origin. The vertices are already in world space, so the axes
                // only tell whether the object was mirrored, which turns its faces inside out.
                if let Some(axes) = slice(bytes, chunk.body, 48, chunk.end) {
                    let m = [0, 12, 24].map(|row| [0, 4, 8].map(|at| f32_in(axes, row + at)));
                    mirrored = determinant(&m) < 0.0;
                }
            }
            _ => {}
        }
    }

    let base = mesh.positions.len() as u32;
    for (face, color) in faces.iter().zip(colors) {
        if let Some(index) = face.iter().find(|&&index| index as usize >= vertices.len()) {
            let name = String::from_utf8_lossy(name);
            let what = format!("a face of \"{name}\" refers to vertex {index}, but that object has only {}", vertices.len());
            return Err(damaged(faces_start, &what));
        }
        if mesh.triangles.len() >= MAX_TRIANGLES {
            return Err(too_many_triangles());
        }
        let [a, b, c] = face.map(|index| base + index as u32);
        mesh.triangles.push(if mirrored { [a, c, b] } else { [a, b, c] });
        mesh.colors.push(color);
    }
    mesh.positions.extend(vertices);
    Ok(())
}

fn damaged(offset: usize, what: &str) -> String {
    format!("The file is damaged at byte {offset}: {what}")
}

/// `len` bytes at `from`, when they lie before `end` and inside the file.
fn slice(bytes: &[u8], from: usize, len: usize, end: usize) -> Option<&[u8]> {
    let to = from.checked_add(len).filter(|&to| to <= end)?;
    bytes.get(from..to)
}

/// The items of a list written as a u16 count followed by that many items of `item_len` bytes.
fn counted(bytes: &[u8], from: usize, item_len: usize, end: usize) -> Option<&[u8]> {
    let count = u16_at(slice(bytes, from, 2, end)?, 0)? as usize;
    slice(bytes, from + 2, count * item_len, end)
}

/// A zero-terminated name starting at `from`, and the offset just after its terminator.
fn name_at(bytes: &[u8], from: usize, end: usize) -> Result<(&[u8], usize), String> {
    let field = bytes.get(from..end).unwrap_or(&[]);
    let len = field.iter().position(|&b| b == 0).ok_or_else(|| damaged(from, "a name has no end"))?;
    Ok((&field[..len], from + len + 1))
}

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    let pair = bytes.get(at..at.checked_add(2)?)?;
    Some(u16::from_le_bytes([pair[0], pair[1]]))
}

/// The f32 at `at` in a slice the caller has already measured.
fn f32_in(bytes: &[u8], at: usize) -> f32 {
    f32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{damage, Rng};

    const KEYFRAMER: u16 = 0xB000;
    const SMOOTHING_GROUPS: u16 = 0x4150;
    const GREY: [u8; 3] = DEFAULT_COLOR;

    fn text(name: &str) -> Vec<u8> {
        [name.as_bytes(), &[0]].concat()
    }

    fn material(name: &str, color: &[u8]) -> Vec<u8> {
        chunk(MATERIAL, &[chunk(MATERIAL_NAME, &text(name)), chunk(MATERIAL_DIFFUSE, color)].concat())
    }

    fn vertices(positions: &[[f32; 3]]) -> Vec<u8> {
        let mut body = (positions.len() as u16).to_le_bytes().to_vec();
        body.extend(positions.iter().flatten().flat_map(|v| v.to_le_bytes()));
        chunk(VERTICES, &body)
    }

    /// Faces, then for each (material, face numbers) a list saying which faces use it.
    fn faces(faces: &[[u16; 3]], materials: &[(&str, &[u16])]) -> Vec<u8> {
        let mut body = (faces.len() as u16).to_le_bytes().to_vec();
        body.extend(faces.iter().flat_map(|f| [f[0], f[1], f[2], 7]).flat_map(u16::to_le_bytes));
        for (name, list) in materials {
            let mut entry = text(name);
            entry.extend((list.len() as u16).to_le_bytes());
            entry.extend(list.iter().flat_map(|face| face.to_le_bytes()));
            body.extend(chunk(FACE_MATERIAL, &entry));
        }
        body.extend(chunk(SMOOTHING_GROUPS, &vec![0; faces.len() * 4]));
        chunk(FACES, &body)
    }

    fn object(name: &str, parts: &[Vec<u8>]) -> Vec<u8> {
        chunk(OBJECT, &[text(name), chunk(TRIMESH, &parts.concat())].concat())
    }

    fn file(editor_parts: &[Vec<u8>]) -> Vec<u8> {
        chunk(MAIN, &[chunk(FILE_VERSION, &3u32.to_le_bytes()), chunk(EDITOR, &editor_parts.concat())].concat())
    }

    const SQUARE: [[f32; 3]; 4] = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]];
    const SQUARE_FACES: [[u16; 3]; 2] = [[0, 1, 2], [0, 2, 3]];

    fn square_file() -> Vec<u8> {
        file(&[object("Square", &[vertices(&SQUARE), faces(&SQUARE_FACES, &[])])])
    }

    fn corners(mesh: &Mesh) -> Vec<[[f32; 3]; 3]> {
        (0..mesh.triangles.len()).map(|at| mesh.corners(at).unwrap()).collect()
    }

    #[test]
    fn one_written_object_reads_back() {
        let mut mesh = Mesh::default();
        mesh.push_cuboid([-1.0, 0.5, 0.0], [2.0, 3.0, 1.25], [10, 20, 30]);
        mesh.push_quad([[0.0; 3], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]], [200, 100, 0]);
        mesh.push_triangle([5.0, 5.0, 5.0], [6.0, 5.0, 5.0], [5.0, 6.0, 5.5], [10, 20, 30]);
        let bytes = write(&mesh, "Stage");
        assert_eq!(bytes[..2], [0x4D, 0x4D]);
        assert_eq!(u32::from_le_bytes([bytes[2], bytes[3], bytes[4], bytes[5]]) as usize, bytes.len());
        assert_eq!(bytes.windows(6).filter(|w| w == b"Stage\0").count(), 1, "one object");
        // Vertices are numbered afresh in the order the faces use them, so compare what is drawn.
        let read = parse(&bytes).unwrap();
        assert_eq!(corners(&read), corners(&mesh));
        assert_eq!(read.colors, mesh.colors);
        assert_eq!(read.positions.len(), mesh.positions.len(), "shared corners stay shared");
        assert_eq!(write(&mesh, "Stage"), bytes, "the same bytes every time");
        assert!(parse(&write(&mesh, "A very long object name")).is_ok());
    }

    #[test]
    fn two_objects_with_two_materials_and_face_lists() {
        let roof: Vec<[f32; 3]> = SQUARE.iter().map(|p| [p[0], p[1], 3.0]).collect();
        let bytes = file(&[
            material("floor", &chunk(COLOR_BYTES, &[60, 50, 40])),
            material("paint", &chunk(COLOR_BYTES, &[200, 210, 220])),
            object("Floor", &[vertices(&SQUARE), faces(&SQUARE_FACES, &[("floor", &[0, 1])])]),
            object(
                "Roof",
                &[
                    vertices(&roof),
                    faces(&[[0, 2, 1], [0, 3, 2], [1, 2, 3]], &[("paint", &[2]), ("floor", &[0]), ("nothing", &[1])]),
                ],
            ),
        ]);
        let mesh = parse(&bytes).unwrap();
        assert_eq!(mesh.positions.len(), 8);
        assert_eq!(mesh.triangles, [[0, 1, 2], [0, 2, 3], [4, 6, 5], [4, 7, 6], [5, 6, 7]]);
        assert_eq!(mesh.colors, [[60, 50, 40], [60, 50, 40], [60, 50, 40], GREY, [200, 210, 220]]);
        assert_eq!(mesh.positions[5], [1.0, 0.0, 3.0]);
        assert_eq!(mesh.validate(), Ok(()));
    }

    #[test]
    fn materials_may_follow_the_objects_that_use_them() {
        let bytes = file(&[
            object("Square", &[vertices(&SQUARE), faces(&SQUARE_FACES, &[("late", &[1])])]),
            material("late", &chunk(COLOR_BYTES, &[1, 2, 3])),
        ]);
        assert_eq!(parse(&bytes).unwrap().colors, [GREY, [1, 2, 3]]);
    }

    #[test]
    fn colours_as_bytes_or_as_numbers() {
        let floats = |id: u16, rgb: [f32; 3]| chunk(id, &rgb.iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<u8>>());
        let colour_of = |diffuse: Vec<u8>| {
            let square = object("Square", &[vertices(&SQUARE), faces(&SQUARE_FACES, &[("m", &[0])])]);
            parse(&file(&[material("m", &diffuse), square])).unwrap().colors[0]
        };
        assert_eq!(colour_of(chunk(COLOR_BYTES, &[12, 34, 56])), [12, 34, 56]);
        assert_eq!(colour_of(floats(COLOR_FLOATS, [1.0, 0.5, 0.0])), [255, 128, 0]);
        assert_eq!(colour_of(floats(COLOR_FLOATS, [7.0, -1.0, f32::NAN])), [255, 0, 0]);
        assert_eq!(colour_of(chunk(COLOR_BYTES_GAMMA, &[9, 8, 7])), [9, 8, 7]);
        assert_eq!(colour_of(floats(COLOR_FLOATS_GAMMA, [0.0, 1.0, 0.2])), [0, 255, 51]);
        // The plain form is preferred to the gamma-corrected one, whichever comes first.
        assert_eq!(colour_of([chunk(COLOR_BYTES_GAMMA, &[9, 8, 7]), chunk(COLOR_BYTES, &[1, 2, 3])].concat()), [1, 2, 3]);
        assert_eq!(
            colour_of([floats(COLOR_FLOATS, [0.2, 0.4, 0.6]), floats(COLOR_FLOATS_GAMMA, [1.0, 1.0, 1.0])].concat()),
            [51, 102, 153]
        );
        assert_eq!(colour_of(chunk(0x0030, &[50, 0])), GREY, "a colour in a form this reader does not know");
    }

    #[test]
    fn a_mirrored_object_has_its_faces_turned() {
        let axes = |x_axis: [f32; 3]| {
            let rows = [x_axis, [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [5.0, 6.0, 7.0]];
            chunk(LOCAL_AXES, &rows.iter().flatten().flat_map(|v| v.to_le_bytes()).collect::<Vec<u8>>())
        };
        let read =
            |axes: Vec<u8>| parse(&file(&[object("Square", &[vertices(&SQUARE), axes, faces(&SQUARE_FACES, &[])])])).unwrap();
        let plain = read(axes([1.0, 0.0, 0.0]));
        assert_eq!(plain.triangles, [[0, 1, 2], [0, 2, 3]]);
        // The origin in the axes is not applied: 3DS stores the vertices already in place.
        assert_eq!(plain.positions, SQUARE);
        let mirrored = read(axes([-1.0, 0.0, 0.0]));
        assert_eq!(mirrored.triangles, [[0, 2, 1], [0, 3, 2]]);
        assert_eq!(mirrored.positions, SQUARE);
        // The axes may come after the faces, and a record that is cut short is passed over.
        let late =
            parse(&file(&[object("Square", &[vertices(&SQUARE), faces(&SQUARE_FACES, &[]), axes([-1.0, 0.0, 0.0])])])).unwrap();
        assert_eq!(late.triangles, mirrored.triangles);
        assert_eq!(read(chunk(LOCAL_AXES, &[0; 20])).triangles, plain.triangles);
    }

    #[test]
    fn sections_it_does_not_need_are_skipped() {
        let keyframer =
            chunk(KEYFRAMER, &[chunk(0xB00A, &[1, 2, 3, 4, 5]), chunk(0xB002, &chunk(0xB010, b"Square\0\0\0\0\0\0\0"))].concat());
        let editor = [
            chunk(MESH_VERSION, &3u32.to_le_bytes()),
            chunk(MASTER_SCALE, &0.0254f32.to_le_bytes()),
            chunk(0x1200, &[0; 7]),
            object("Lamp", &[chunk(0x4600, &[0; 12])]),
            chunk(OBJECT, &[text("Camera"), chunk(0x4700, &[0; 32])].concat()),
            object("Square", &[chunk(0x4165, &[3]), vertices(&SQUARE), chunk(0x4140, &[0; 34]), faces(&SQUARE_FACES, &[])]),
        ]
        .concat();
        let bytes = chunk(
            MAIN,
            &[chunk(FILE_VERSION, &3u32.to_le_bytes()), chunk(0x1234, &[]), chunk(EDITOR, &editor), keyframer].concat(),
        );
        let mesh = parse(&bytes).unwrap();
        assert_eq!((mesh.positions.len(), mesh.triangles.len()), (4, 2));
        // Whatever follows the main section is not part of the model.
        assert_eq!(parse(&[bytes, vec![1, 2, 3]].concat()).unwrap(), mesh);
    }

    #[test]
    fn files_that_are_not_3ds_are_refused() {
        for bytes in [&b""[..], b"MM", b"\x4D\x4D\x06\x00\x00", b"# an OBJ file\nv 0 0 0\n", b"PTZM\x01\x00\x00\x00"] {
            assert_eq!(parse(bytes).unwrap_err(), "This is not a 3DS file");
        }
    }

    #[test]
    fn damage_is_reported_with_its_place() {
        let good = square_file();
        assert!(parse(&good).is_ok());
        let fails = |bytes: &[u8]| parse(bytes).unwrap_err();

        assert_eq!(fails(&good[..good.len() - 1]), "The file is damaged at byte 0: a section runs past the end");
        assert_eq!(fails(&good[..40]), "The file is damaged at byte 0: a section runs past the end");

        // The editor section starts after the main heading (6 bytes) and the version section (10 bytes).
        let editor = 16;
        let mut short = good.clone();
        short[editor + 2..editor + 6].copy_from_slice(&5u32.to_le_bytes());
        assert_eq!(fails(&short), "The file is damaged at byte 16: a section is shorter than its own heading");

        let mut long = good.clone();
        long[editor + 2..editor + 6].copy_from_slice(&(good.len() as u32).to_le_bytes());
        assert_eq!(fails(&long), "The file is damaged at byte 16: a section runs past the end");

        // The object section follows the editor heading; its trimesh follows the object's heading and "Square\0".
        let trimesh = editor + 6 + 6 + 7;
        let mut past_parent = good.clone();
        let trimesh_length = u32::from_le_bytes([good[trimesh + 2], good[trimesh + 3], good[trimesh + 4], good[trimesh + 5]]);
        past_parent[trimesh + 2..trimesh + 6].copy_from_slice(&(trimesh_length + 1).to_le_bytes());
        assert_eq!(fails(&past_parent), "The file is damaged at byte 35: a section runs past the end");
        // The same mistake where more of the file follows names the enclosing section instead.
        let mut long_trimesh = chunk(TRIMESH, &vertices(&SQUARE));
        long_trimesh[2] += 1;
        let followed = file(&[
            chunk(OBJECT, &[text("Square"), long_trimesh].concat()),
            object("Square", &[vertices(&SQUARE), faces(&SQUARE_FACES, &[])]),
        ]);
        assert_eq!(fails(&followed), "The file is damaged at byte 35: a section runs past the section that holds it");

        let unnamed = file(&[chunk(OBJECT, b"Squ")]);
        assert_eq!(fails(&unnamed), "The file is damaged at byte 28: a name has no end");

        let stray = chunk(MAIN, &[chunk(EDITOR, &[]), vec![1, 2, 3]].concat());
        assert_eq!(fails(&stray), "The file is damaged at byte 12: a section is cut short");
    }

    #[test]
    fn lists_that_do_not_fit_their_section_are_refused() {
        let fails = |parts: &[Vec<u8>]| parse(&file(&[object("Box", parts)])).unwrap_err();
        let mut cut_vertices = vertices(&SQUARE);
        cut_vertices[6] = 5;
        assert_eq!(
            fails(&[cut_vertices, faces(&SQUARE_FACES, &[])]),
            "The file is damaged at byte 38: a list of vertices is cut short"
        );
        assert!(fails(&[chunk(VERTICES, &[9]), faces(&SQUARE_FACES, &[])]).ends_with("a list of vertices is cut short"));

        let mut cut_faces = chunk(FACES, &[2, 0, 0, 0, 1, 0, 2, 0, 7, 0]);
        assert_eq!(
            fails(&[vertices(&SQUARE), cut_faces.clone()]),
            "The file is damaged at byte 94: a list of faces is cut short"
        );
        cut_faces[6] = 1;
        assert!(parse(&file(&[object("Box", &[vertices(&SQUARE), cut_faces])])).is_ok());

        let out_of_range = fails(&[vertices(&SQUARE), faces(&[[0, 1, 2], [0, 2, 4]], &[])]);
        assert_eq!(
            out_of_range,
            "The file is damaged at byte 94: a face of \"Box\" refers to vertex 4, but that object has only 4"
        );
        assert!(fails(&[faces(&SQUARE_FACES, &[])]).contains("refers to vertex 0, but that object has only 0"));

        let bad_material = fails(&[vertices(&SQUARE), faces(&SQUARE_FACES, &[("m", &[0, 2])])]);
        assert!(bad_material.ends_with("a material is given to face 2, but the object has only 2"), "{bad_material}");

        let not_a_number = fails(&[vertices(&[[0.0, f32::NAN, 0.0]]), faces(&[], &[])]);
        assert_eq!(not_a_number, "The file is damaged at byte 38: a vertex position is not a usable number");

        let cut_colour = parse(&file(&[material("m", &chunk(COLOR_BYTES, &[1, 2])), object("Box", &[])])).unwrap_err();
        assert_eq!(cut_colour, "The file is damaged at byte 42: a colour is cut short");
    }

    #[test]
    fn files_without_faces_are_refused() {
        let empty = [
            chunk(MAIN, &[]),
            file(&[]),
            file(&[material("m", &chunk(COLOR_BYTES, &[1, 2, 3]))]),
            file(&[object("Points", &[vertices(&SQUARE)])]),
            file(&[object("Nothing", &[vertices(&SQUARE), faces(&[], &[])])]),
            write(&Mesh::default(), "Empty"),
        ];
        for bytes in empty {
            assert_eq!(parse(&bytes).unwrap_err(), "The file has no faces to draw");
        }
    }

    #[test]
    fn a_mesh_too_big_for_one_object_is_split() {
        let mut mesh = Mesh::default();
        for at in 0..30_000 {
            let (x, y) = ((at % 200) as f32, (at / 200) as f32);
            let color = [(at % 7) as u8 * 30, (at % 3) as u8 * 100, 90];
            mesh.push_triangle([x, y, 0.0], [x + 0.9, y, 0.0], [x, y + 0.9, 0.5], color);
        }
        let bytes = write(&mesh, "Hall");
        assert_eq!(bytes.windows(5).filter(|w| w == b"Hall\0").count(), 1);
        assert_eq!(bytes.windows(6).filter(|w| w == b"Hall2\0").count(), 1, "90,000 corners need a second object");
        assert_eq!(parse(&bytes).unwrap(), mesh);

        // Many faces sharing few corners hit the limit on faces instead.
        let mut fan = Mesh { positions: (0..300).map(|at| [at as f32, (at * at % 17) as f32, 1.0]).collect(), ..Mesh::default() };
        for at in 0..70_000u32 {
            fan.triangles.push([at % 300, (at * 7 + 1) % 300, (at * 13 + 2) % 300]);
            fan.colors.push([(at % 5) as u8, 0, 0]);
        }
        let read = parse(&write(&fan, "Fan")).unwrap();
        assert_eq!(corners(&read), corners(&fan));
        assert_eq!(read.colors, fan.colors);
    }

    /// Offsets of the heading of every section in a file made by `write`.
    fn headings(bytes: &[u8], from: usize, end: usize, found: &mut Vec<usize>) {
        let mut chunks = Chunks { bytes, at: from, end };
        while let Ok(Some(chunk)) = chunks.next() {
            found.push(chunk.start);
            let name_len = || bytes[chunk.body..chunk.end].iter().position(|&b| b == 0).unwrap() + 1;
            match chunk.id {
                MAIN | EDITOR | MATERIAL | MATERIAL_DIFFUSE | TRIMESH => headings(bytes, chunk.body, chunk.end, found),
                OBJECT => headings(bytes, chunk.body + name_len(), chunk.end, found),
                FACES => {
                    let count = u16_at(bytes, chunk.body).unwrap() as usize;
                    headings(bytes, chunk.body + 2 + count * 8, chunk.end, found);
                }
                _ => {}
            }
        }
    }

    #[test]
    fn damaged_3ds_files_never_panic() {
        let mut mesh = Mesh::default();
        mesh.push_cuboid([0.0; 3], [1.0, 2.0, 3.0], [10, 20, 30]);
        mesh.push_cuboid([5.0; 3], [6.0; 3], [200, 0, 255]);
        let good = [write(&mesh, "Room"), square_file()];
        let mut rng = Rng(0x1234_5678_9ABC_DEF1);
        let mut read = 0;
        for round in 0..6000 {
            let valid = &good[round % 2];
            let mut bytes = damage(valid, &mut rng);
            if round % 3 == 0 {
                // Spoil the length of a real section, or the count at the head of one of its lists.
                let mut found = Vec::new();
                headings(valid, 0, valid.len(), &mut found);
                let at = found[rng.below(found.len())] + [2, 6][rng.below(2)];
                let value = [0, 5, 6, 7, 0xFFFF, valid.len() as u32, u32::MAX, rng.next() as u32 % 200][rng.below(8)];
                let field = at.min(bytes.len().saturating_sub(4))..(at + 4).min(bytes.len());
                let len = field.len();
                bytes[field].copy_from_slice(&value.to_le_bytes()[..len]);
            }
            if let Ok(mesh) = parse(&bytes) {
                assert_eq!(mesh.validate(), Ok(()));
                read += 1;
            }
        }
        assert!(read > 0, "some damaged files should still be readable");
    }
}
