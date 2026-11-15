//! A panel's description of itself: the JSON after `_panelTopology_HWC=`
//! (reference chapter 9). Type numbers mean nothing outside the topology they
//! came in, so everything is resolved to what a component sends and shows.

use serde::Deserialize;
use std::collections::BTreeMap;

/// What a component sends, from the `in` key of its type.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Input {
    /// `b`, `b4`, `b2v`, `b2h`, `gpi`: reports Down and Up.
    Button,
    /// `pb`, `p`, `pi`: reports steps; `push` when it can also be pressed.
    Encoder { push: bool },
    /// `iv`, `ih`, `ir`, `i`: a sprung axis, -500 to 500.
    Intensity,
    /// `av`, `ah`, `ar`, `a`: an absolute position, 0 to 1000.
    Absolute,
    /// Shows something, sends nothing.
    None,
}

impl Input {
    pub fn word(self) -> &'static str {
        match self {
            Input::Button => "button",
            Input::Encoder { push: true } => "encoder with push",
            Input::Encoder { push: false } => "encoder",
            Input::Intensity => "joystick axis",
            Input::Absolute => "fader",
            Input::None => "display",
        }
    }

    fn of(code: &str) -> Input {
        match code.split(',').next().unwrap_or("").trim() {
            "b" | "b4" | "b2v" | "b2h" | "gpi" => Input::Button,
            "pb" => Input::Encoder { push: true },
            "p" | "pi" => Input::Encoder { push: false },
            "iv" | "ih" | "ir" | "i" => Input::Intensity,
            "av" | "ah" | "ar" | "a" => Input::Absolute,
            _ => Input::None,
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct Component {
    pub id: u32,
    /// The panel's own label for it, with `|` line breaks made spaces.
    pub label: String,
    pub input: Input,
    /// It has a light that takes `HWC#` and `HWCc#`.
    pub light: bool,
    /// Display size in pixels, when it has one.
    pub display: Option<(u32, u32)>,
    pub description: String,
}

#[derive(Clone, PartialEq, Debug, Default)]
pub struct Topology {
    pub components: Vec<Component>,
}

#[derive(Deserialize, Default, Clone)]
struct RawDisp {
    #[serde(default)]
    w: u32,
    #[serde(default)]
    h: u32,
}

#[derive(Deserialize, Default, Clone)]
struct RawType {
    #[serde(default, rename = "in")]
    input: String,
    #[serde(default)]
    out: String,
    #[serde(default)]
    desc: String,
    #[serde(default)]
    disp: Option<RawDisp>,
}

#[derive(Deserialize)]
struct RawComponent {
    id: u32,
    #[serde(default)]
    txt: String,
    #[serde(default, rename = "type")]
    kind: serde_json::Value,
    #[serde(default, rename = "typeOverride")]
    over: Option<RawType>,
}

#[derive(Deserialize)]
struct Raw {
    #[serde(default, rename = "HWc")]
    hwc: Vec<RawComponent>,
    #[serde(default, rename = "typeIndex")]
    types: BTreeMap<String, RawType>,
}

impl Topology {
    pub fn parse(json: &str) -> Result<Topology, String> {
        let raw: Raw = serde_json::from_str(json).map_err(|e| format!("The panel's topology could not be read: {e}"))?;
        let mut components = Vec::new();
        for c in raw.hwc {
            let key = match &c.kind {
                serde_json::Value::Number(n) => n.to_string(),
                serde_json::Value::String(s) => s.clone(),
                _ => String::new(),
            };
            // Type 0 is a disabled component (reference 9.2).
            if key == "0" {
                continue;
            }
            let mut kind = raw.types.get(&key).cloned().unwrap_or_default();
            // An override replaces strings when not empty and the display when it has values.
            if let Some(over) = c.over {
                if !over.input.is_empty() {
                    kind.input = over.input;
                }
                if !over.out.is_empty() {
                    kind.out = over.out;
                }
                if !over.desc.is_empty() {
                    kind.desc = over.desc;
                }
                if let Some(d) = over.disp.filter(|d| d.w > 0 && d.h > 0) {
                    kind.disp = Some(d);
                }
            }
            components.push(Component {
                id: c.id,
                label: c.txt.replace('|', " "),
                input: Input::of(&kind.input),
                light: !kind.out.is_empty() && kind.out != "gpo",
                display: kind.disp.filter(|d| d.w > 0 && d.h > 0).map(|d| (d.w, d.h)),
                description: kind.desc,
            });
        }
        components.sort_by_key(|c| c.id);
        Ok(Topology { components })
    }

    pub fn get(&self, id: u32) -> Option<&Component> {
        self.components.iter().find(|c| c.id == id)
    }
}

/// The PTZ Pro (v2) as SKAARHOJ's Raw Panel Dummies v1.5.6-pre2 reports it for
/// its embedded profile `PTZPROV2`: the line after `_panelTopology_HWC=`.
pub const PTZ_PRO_TOPOLOGY: &str = include_str!("../data/ptz_pro_v2_topology.json");

pub fn ptz_pro() -> Topology {
    Topology::parse(PTZ_PRO_TOPOLOGY).expect("the bundled PTZ Pro topology is valid")
}

#[cfg(test)]
mod tests {
    use super::*;

    // Reference 9.2, the documented excerpt.
    #[test]
    fn the_reference_excerpt_is_read() {
        let json = r#"{"HWc":[
            {"id":1,"x":201,"y":1449,"txt":"PRV 1","type":132},
            {"id":9,"x":180,"y":909,"txt":"Knob A","type":15,
             "typeOverride":{"disp":{"w":64,"h":32,"subidx":0},
             "sub":[{"_":"r","_x":-55,"_y":-550,"_w":190,"_h":95,"rx":5,"ry":5}]}},
            {"id":20,"x":0,"y":0,"txt":"Fader","type":28},
            {"id":21,"x":0,"y":0,"txt":"Tile","type":36},
            {"id":22,"x":0,"y":0,"txt":"Off","type":0}],
          "typeIndex":{
            "15":{"w":160,"out":"rgb","in":"pb","desc":"Encoder"},
            "28":{"w":30,"h":710,"in":"av","ext":"pos","subidx":0,"desc":"Motorized Fader 60mm",
                  "sub":[{"_":"r","_x":-63,"_y":53,"_w":125,"_h":250}]},
            "36":{"w":570,"h":151,"disp":{"w":128,"h":32},"desc":"OLED Display Tile"}}}"#;
        let t = Topology::parse(json).unwrap();
        assert_eq!(t.components.len(), 4, "type 0 is a disabled component");
        let knob = t.get(9).unwrap();
        assert_eq!(knob.input, Input::Encoder { push: true });
        assert_eq!(knob.display, Some((64, 32)), "the override adds a display to this one encoder");
        assert!(knob.light);
        assert_eq!(t.get(20).unwrap().input, Input::Absolute);
        let tile = t.get(21).unwrap();
        assert_eq!((tile.input, tile.display, tile.light), (Input::None, Some((128, 32)), false));
        assert_eq!(t.get(1).unwrap().input, Input::None, "a type the index does not hold sends nothing we know of");
    }

    // The counts are the PTZ Pro datasheet's (SKU PTZ-PRO-V2B): 9 four-way buttons with
    // display, 20 elastomer buttons, 5 encoders, a joystick with three axes and a top button.
    #[test]
    fn the_bundled_ptz_pro_matches_its_datasheet() {
        let t = ptz_pro();
        let count = |f: &dyn Fn(&Component) -> bool| t.components.iter().filter(|c| f(c)).count();
        assert_eq!(t.components.len(), 51);
        assert_eq!(count(&|c| c.input == Input::Button && c.display == Some((64, 32))), 9);
        assert_eq!(count(&|c| c.input == Input::Button && c.display.is_none()), 21, "20 buttons and the joystick's top");
        assert_eq!(count(&|c| c.input == Input::Encoder { push: true }), 5);
        assert_eq!(count(&|c| c.input == Input::Intensity), 3);
        assert_eq!(count(&|c| c.input == Input::None && c.display == Some((128, 32))), 4);
        assert_eq!(count(&|c| c.input == Input::None && c.display == Some((64, 32))), 8);
        assert_eq!(count(&|c| c.display == Some((256, 64))), 1);
        assert_eq!(t.get(56).unwrap().label, "CAM 9");
        assert_eq!(t.get(3).unwrap().label, "Rotate");
    }
}
