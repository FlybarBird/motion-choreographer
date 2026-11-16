//! The ASCII form of SKAARHOJ's Raw Panel protocol: one command per line.
//!
//! Written from the "Raw Panel Protocol Reference" (revision October 28 2026)
//! and rawpanel-lib's converters. Section numbers in comments are the
//! reference's. Nothing here opens a socket.

/// What an operator did to one hardware component (reference 5.1).
#[derive(Clone, PartialEq, Debug)]
pub enum Trigger {
    Down,
    Up,
    /// Legacy UniSketch: a press and release in one.
    Press,
    /// Encoder steps since the last event; negative is counter-clockwise.
    Enc(i32),
    /// A sprung component: -500 to 500, 0 at rest.
    Speed(i32),
    /// An absolute component: 0 to 1000.
    Abs(i32),
    SpeedVector(Vec<i32>),
    AbsVector(Vec<i32>),
    Raw(i32),
    Text(String),
}

/// One line from the panel.
#[derive(Clone, PartialEq, Debug)]
pub enum FromPanel {
    /// `edge` is 0 for a component without edges; 1 top, 2 left, 4 bottom, 8 right, 16 a pushed encoder.
    Event {
        hwc: u32,
        edge: u8,
        trigger: Trigger,
    },
    Ping,
    Ack,
    Nack,
    /// UniSketch flow control: hold back until `Ready` or `Ping`.
    Busy,
    Ready,
    /// Client mode: the panel asks to be activated.
    List,
    /// `to` is 0 when the component is not available, otherwise the number its events and feedback use.
    Map {
        hwc: u32,
        to: u32,
        offscreen: bool,
    },
    Model(String),
    Serial(String),
    Version(String),
    Name(String),
    Platform(String),
    PanelType(String),
    Support(Vec<String>),
    TopologySvg(String),
    /// The JSON after `_panelTopology_HWC=`.
    Topology(String),
    HeartBeatTimer(u32),
    SleepTimer(u32),
    Sleeping(bool),
    Msg(String),
    ErrorMsg(String),
    /// A line this program has no use for.
    Other(String),
}

fn number(s: &str) -> Option<i32> {
    s.trim().parse().ok()
}

fn event(rest: &str) -> Option<FromPanel> {
    let (target, what) = rest.split_once('=')?;
    let (hwc, edge) = match target.split_once('.') {
        Some((hwc, edge)) => (hwc.parse().ok()?, edge.parse().ok()?),
        None => (target.parse().ok()?, 0),
    };
    let (word, value) = match what.split_once(':') {
        Some((word, value)) => (word, Some(value)),
        None => (what, None),
    };
    let many = |v: &str| v.split(',').map(number).collect::<Option<Vec<i32>>>();
    let trigger = match (word, value) {
        ("Down", None) => Trigger::Down,
        ("Up", None) => Trigger::Up,
        ("Press", None) => Trigger::Press,
        ("Enc", Some(v)) => Trigger::Enc(number(v)?),
        ("Raw", Some(v)) => Trigger::Raw(number(v)?),
        ("Speed", Some(v)) if v.contains(',') => Trigger::SpeedVector(many(v)?),
        ("Speed", Some(v)) => Trigger::Speed(number(v)?),
        ("Abs", Some(v)) if v.contains(',') => Trigger::AbsVector(many(v)?),
        ("Abs", Some(v)) => Trigger::Abs(number(v)?),
        ("Text", Some(v)) => Trigger::Text(v.to_string()),
        _ => return None,
    };
    Some(FromPanel::Event { hwc, edge, trigger })
}

/// Read one line, without its line ending.
pub fn parse(line: &str) -> FromPanel {
    let line = line.trim_end_matches(['\r', '\n']);
    let other = || FromPanel::Other(line.to_string());
    match line {
        "ping" => return FromPanel::Ping,
        "ack" => return FromPanel::Ack,
        "nack" => return FromPanel::Nack,
        "BSY" => return FromPanel::Busy,
        "RDY" => return FromPanel::Ready,
        "list" => return FromPanel::List,
        _ => {}
    }
    if let Some(rest) = line.strip_prefix("HWC#") {
        return event(rest).unwrap_or_else(other);
    }
    let Some((key, value)) = line.split_once('=') else {
        return other();
    };
    match key {
        "map" => {
            let parsed = value.split_once(':').and_then(|(a, b)| Some((a.parse::<u32>().ok()?, b.parse::<u32>().ok()?)));
            match parsed {
                Some((hwc, to)) => FromPanel::Map { hwc, to: to & 0x7fff_ffff, offscreen: to & 0x8000_0000 != 0 },
                None => other(),
            }
        }
        "_model" => FromPanel::Model(value.into()),
        "_serial" => FromPanel::Serial(value.into()),
        "_version" => FromPanel::Version(value.into()),
        "_name" => FromPanel::Name(value.into()),
        "_platform" => FromPanel::Platform(value.into()),
        "_panelType" => FromPanel::PanelType(value.into()),
        "_support" => FromPanel::Support(value.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()),
        "_panelTopology_svgbase" => FromPanel::TopologySvg(value.into()),
        "_panelTopology_HWC" => FromPanel::Topology(value.into()),
        "_heartBeatTimer" => value.parse().map(FromPanel::HeartBeatTimer).unwrap_or_else(|_| other()),
        "_sleepTimer" => value.parse().map(FromPanel::SleepTimer).unwrap_or_else(|_| other()),
        "_isSleeping" => FromPanel::Sleeping(value.trim() == "1"),
        "Msg" => FromPanel::Msg(value.into()),
        "ErrorMsg" => FromPanel::ErrorMsg(value.into()),
        _ => other(),
    }
}

/// How a button's light is driven (reference 4.1: "in practice you send 0, 36 and 5").
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Lit {
    #[default]
    Off,
    Dim,
    On,
    /// On, blinking at 2 Hz (blink pattern 2, reference 6.2).
    Blink,
}

impl Lit {
    pub fn value(self) -> u32 {
        match self {
            Lit::Off => 0,
            Lit::Dim => 5,
            Lit::On => 36,
            Lit::Blink => 36 + 2 * 256,
        }
    }

    pub fn from_value(v: u32) -> Lit {
        match (v & 0xf, (v >> 8) & 0xf) {
            (0, _) => Lit::Off,
            (5, _) => Lit::Dim,
            (_, 0) => Lit::On,
            _ => Lit::Blink,
        }
    }
}

/// The part of SKAARHOJ's palette this program uses (reference 6.1).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Color {
    #[default]
    Default,
    White,
    Red,
    Amber,
    Blue,
    Green,
}

impl Color {
    pub const ALL: [Color; 6] = [Color::Default, Color::White, Color::Red, Color::Amber, Color::Blue, Color::Green];

    fn index(self) -> u32 {
        match self {
            Color::Default => 0,
            Color::White => 2,
            Color::Red => 4,
            Color::Amber => 8,
            Color::Blue => 11,
            Color::Green => 15,
        }
    }

    /// The `HWCc` value: the palette index with bit 7 set.
    pub fn value(self) -> u32 {
        128 + self.index()
    }

    pub fn from_value(v: u32) -> Option<Color> {
        Color::ALL.into_iter().find(|c| c.value() == v)
    }

    pub fn name(self) -> &'static str {
        match self {
            Color::Default => "default",
            Color::White => "white",
            Color::Red => "red",
            Color::Amber => "amber",
            Color::Blue => "blue",
            Color::Green => "green",
        }
    }
}

/// What a display tile shows: a title and up to two lines (reference 7.3).
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Text {
    pub title: String,
    /// The thin title line: what a press will do. Otherwise the solid bar: a current value.
    pub label: bool,
    pub line1: String,
    pub line2: String,
    /// Print `line1` in the small font even when it stands alone.
    pub small: bool,
}

impl Text {
    pub fn is_empty(&self) -> bool {
        self.title.is_empty() && self.line1.is_empty() && self.line2.is_empty()
    }

    /// Everything on the tile as one string, for reading in tests and on a terminal.
    pub fn all(&self) -> String {
        [self.title.as_str(), self.line1.as_str(), self.line2.as_str()]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" / ")
    }
}

/// The longest `HWCt` string a panel takes (reference 7.3).
pub const TEXT_LIMIT: usize = 63;

/// Printable ASCII only, and no field separator.
pub fn printable(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '|' => '/',
            ' '..='~' => c,
            _ => '?',
        })
        .collect()
}

fn clip(s: &str, max: usize) -> String {
    printable(s).chars().take(max).collect()
}

pub fn hwc_mode(hwc: u32, lit: Lit) -> String {
    format!("HWC#{hwc}={}", lit.value())
}

pub fn hwc_color(hwc: u32, color: Color) -> String {
    format!("HWCc#{hwc}={}", color.value())
}

/// `HWCt#n=|||title|isLabel|line1|line2|value2`, trailing empty fields left off.
/// An empty text blanks the tile. Lines are shortened until the string fits.
pub fn hwc_text(hwc: u32, text: &Text) -> String {
    if text.is_empty() {
        return format!("HWCt#{hwc}=");
    }
    let build = |title: &str, line1: &str, line2: &str| {
        let label = if text.label { "1" } else { "" };
        let value2 = if text.small && line2.is_empty() && !line1.is_empty() { "0" } else { "" };
        let mut fields = vec!["", "", "", title, label, line1, line2, value2];
        while fields.last().is_some_and(|f| f.is_empty()) {
            fields.pop();
        }
        fields.join("|")
    };
    let (mut title, mut line1, mut line2) = (clip(&text.title, 24), clip(&text.line1, 24), clip(&text.line2, 24));
    let mut body = build(&title, &line1, &line2);
    while body.len() > TEXT_LIMIT {
        let longest = [line2.len(), line1.len(), title.len()].into_iter().max().unwrap_or(0);
        if longest == 0 {
            break;
        }
        if line2.len() == longest {
            line2.pop();
        } else if line1.len() == longest {
            line1.pop();
        } else {
            title.pop();
        }
        body = build(&title, &line1, &line2);
    }
    format!("HWCt#{hwc}={body}")
}

/// Read a `HWCt` value back into a `Text`. Used by the stand-in panel.
pub fn parse_text(value: &str) -> Text {
    let fields: Vec<&str> = value.split('|').collect();
    let at = |i: usize| fields.get(i).copied().unwrap_or("");
    Text {
        title: at(3).to_string(),
        label: at(4) == "1",
        line1: at(5).to_string(),
        line2: at(6).to_string(),
        small: !at(7).is_empty(),
    }
}

/// A feedback command, as the stand-in panel reads it.
#[derive(Clone, PartialEq, Debug)]
pub enum ToPanel {
    Mode { hwcs: Vec<u32>, value: u32 },
    Color { hwcs: Vec<u32>, value: u32 },
    Text { hwcs: Vec<u32>, value: String },
    Extended { hwcs: Vec<u32>, value: u32 },
    List,
    Topology,
    Map,
    Ping,
    Ack,
    Nack,
    ActivePanel,
    HeartBeatTimer(u32),
    SleepTimer(u32),
    GetSleepTimer,
    WakeUp,
    Clear,
    ClearLeds,
    ClearDisplays,
    Brightness(String),
    Unknown(String),
}

/// Read one line the system sent, as a panel would.
pub fn parse_to_panel(line: &str) -> ToPanel {
    let line = line.trim_end_matches(['\r', '\n']);
    match line {
        "list" => return ToPanel::List,
        "PanelTopology?" => return ToPanel::Topology,
        "map" => return ToPanel::Map,
        "ping" => return ToPanel::Ping,
        "ack" => return ToPanel::Ack,
        "nack" => return ToPanel::Nack,
        "ActivePanel=1" => return ToPanel::ActivePanel,
        "SleepTimer?" => return ToPanel::GetSleepTimer,
        "WakeUp!" => return ToPanel::WakeUp,
        "Clear" => return ToPanel::Clear,
        "ClearLEDs" => return ToPanel::ClearLeds,
        "ClearDisplays" => return ToPanel::ClearDisplays,
        _ => {}
    }
    let unknown = || ToPanel::Unknown(line.to_string());
    let Some((key, value)) = line.split_once('=') else {
        return unknown();
    };
    let ids = |list: &str| list.split(',').map(|s| s.parse::<u32>().ok()).collect::<Option<Vec<u32>>>();
    // Longest prefix first: `HWCc#` and `HWCt#` also start with `HWC`.
    for prefix in ["HWCc#", "HWCt#", "HWCx#", "HWC#"] {
        let Some(list) = key.strip_prefix(prefix) else { continue };
        let Some(hwcs) = ids(list) else { return unknown() };
        return match prefix {
            "HWCt#" => ToPanel::Text { hwcs, value: value.to_string() },
            "HWCc#" => value.parse().map(|value| ToPanel::Color { hwcs, value }).unwrap_or_else(|_| unknown()),
            "HWCx#" => value.parse().map(|value| ToPanel::Extended { hwcs, value }).unwrap_or_else(|_| unknown()),
            _ => value.parse().map(|value| ToPanel::Mode { hwcs, value }).unwrap_or_else(|_| unknown()),
        };
    }
    match key {
        "HeartBeatTimer" => value.parse().map(ToPanel::HeartBeatTimer).unwrap_or_else(|_| unknown()),
        "SleepTimer" => value.parse().map(ToPanel::SleepTimer).unwrap_or_else(|_| unknown()),
        "PanelBrightness" => ToPanel::Brightness(value.to_string()),
        _ => unknown(),
    }
}

/// How an event is written on the wire, as a panel sends it.
pub fn format_event(hwc: u32, edge: u8, trigger: &Trigger) -> String {
    let target = if edge == 0 { format!("HWC#{hwc}") } else { format!("HWC#{hwc}.{edge}") };
    let list = |v: &[i32]| v.iter().map(|n| n.to_string()).collect::<Vec<_>>().join(",");
    match trigger {
        Trigger::Down => format!("{target}=Down"),
        Trigger::Up => format!("{target}=Up"),
        Trigger::Press => format!("{target}=Press"),
        Trigger::Enc(n) => format!("{target}=Enc:{n}"),
        Trigger::Speed(n) => format!("{target}=Speed:{n}"),
        Trigger::Abs(n) => format!("{target}=Abs:{n}"),
        Trigger::SpeedVector(v) => format!("{target}=Speed:{}", list(v)),
        Trigger::AbsVector(v) => format!("{target}=Abs:{}", list(v)),
        Trigger::Raw(n) => format!("{target}=Raw:{n}"),
        Trigger::Text(s) => format!("{target}=Text:{s}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(hwc: u32, edge: u8, trigger: Trigger) -> FromPanel {
        FromPanel::Event { hwc, edge, trigger }
    }

    // Reference 1.1 "A first session" and 5.1 "Events": the lines are SKAARHOJ's own examples.
    #[test]
    fn events_from_the_reference_are_read() {
        assert_eq!(parse("HWC#38=Down"), ev(38, 0, Trigger::Down));
        assert_eq!(parse("HWC#38=Up"), ev(38, 0, Trigger::Up));
        assert_eq!(parse("HWC#35.4=Down"), ev(35, 4, Trigger::Down));
        assert_eq!(parse("HWC#35.4=Up"), ev(35, 4, Trigger::Up));
        assert_eq!(parse("HWC#25=Abs:768"), ev(25, 0, Trigger::Abs(768)));
        assert_eq!(parse("HWC#42=Enc:2"), ev(42, 0, Trigger::Enc(2)));
        assert_eq!(parse("HWC#38=Enc:-2"), ev(38, 0, Trigger::Enc(-2)));
        assert_eq!(parse("HWC#42=Speed:136"), ev(42, 0, Trigger::Speed(136)));
        assert_eq!(parse("HWC#50=Abs:300,700"), ev(50, 0, Trigger::AbsVector(vec![300, 700])));
        assert_eq!(parse("HWC#60=Text:Studio A"), ev(60, 0, Trigger::Text("Studio A".into())));
        assert_eq!(parse("HWC#12=Raw:2048"), ev(12, 0, Trigger::Raw(2048)));
        assert_eq!(parse("HWC#7=Press"), ev(7, 0, Trigger::Press));
    }

    // Lines SKAARHOJ's Raw Panel Dummies v1.5.6-pre2 sent for its PTZ Pro.
    #[test]
    fn events_from_skaarhojs_emulator_are_read() {
        assert_eq!(parse("HWC#1=Speed:-400"), ev(1, 0, Trigger::Speed(-400)));
        assert_eq!(parse("HWC#3=Speed:500"), ev(3, 0, Trigger::Speed(500)));
        assert_eq!(parse("HWC#35=Enc:2"), ev(35, 0, Trigger::Enc(2)));
        assert_eq!(parse("HWC#4=Down\r"), ev(4, 0, Trigger::Down));
    }

    // Reference 5.2 (map), 5.3 (panel information), 5.5 (flow control), 5.6 (status).
    #[test]
    fn housekeeping_from_the_reference_is_read() {
        assert_eq!(parse("map=23:23"), FromPanel::Map { hwc: 23, to: 23, offscreen: false });
        assert_eq!(parse("map=23:0"), FromPanel::Map { hwc: 23, to: 0, offscreen: false });
        assert_eq!(parse("map=201:2147483849"), FromPanel::Map { hwc: 201, to: 201, offscreen: true });
        assert_eq!(parse("_model=SK_RACKFUSIONLIVE"), FromPanel::Model("SK_RACKFUSIONLIVE".into()));
        assert_eq!(parse("_serial=435775"), FromPanel::Serial("435775".into()));
        assert_eq!(parse("_version=v2.4.1"), FromPanel::Version("v2.4.1".into()));
        assert_eq!(parse("_name=Studio A Fusion"), FromPanel::Name("Studio A Fusion".into()));
        assert_eq!(parse("_platform=ibeam"), FromPanel::Platform("ibeam".into()));
        assert_eq!(parse("_panelType=BPI"), FromPanel::PanelType("BPI".into()));
        assert_eq!(
            parse("_support=ASCII,Binary,JSONFeedback,JSONonInbound,JSONonOutbound,Processors"),
            FromPanel::Support(
                ["ASCII", "Binary", "JSONFeedback", "JSONonInbound", "JSONonOutbound", "Processors"].map(String::from).to_vec()
            )
        );
        assert_eq!(parse("ping"), FromPanel::Ping);
        assert_eq!(parse("ack"), FromPanel::Ack);
        assert_eq!(parse("nack"), FromPanel::Nack);
        assert_eq!(parse("BSY"), FromPanel::Busy);
        assert_eq!(parse("RDY"), FromPanel::Ready);
        assert_eq!(parse("list"), FromPanel::List);
        assert_eq!(parse("_heartBeatTimer=3000"), FromPanel::HeartBeatTimer(3000));
        assert_eq!(parse("_sleepTimer=2000"), FromPanel::SleepTimer(2000));
        assert_eq!(parse("_sleepTimer=0"), FromPanel::SleepTimer(0));
        assert_eq!(parse("_isSleeping=1"), FromPanel::Sleeping(true));
        assert_eq!(parse("Msg=Rebooting..."), FromPanel::Msg("Rebooting...".into()));
        assert_eq!(parse("ErrorMsg=Max clients reached"), FromPanel::ErrorMsg("Max clients reached".into()));
        assert_eq!(
            parse(r#"_panelTopology_HWC={"HWc":[{"id":1,"x":201,"y":1449,"txt":"PRV 1","type":132}],"typeIndex":{}}"#),
            FromPanel::Topology(r#"{"HWc":[{"id":1,"x":201,"y":1449,"txt":"PRV 1","type":132}],"typeIndex":{}}"#.into())
        );
    }

    #[test]
    fn lines_that_are_not_understood_are_kept_and_never_panic() {
        for line in ["", "HWC#", "HWC#x=Down", "HWC#12=Sideways", "HWC#12=Enc:two", "map=1", "EnvironmentalHealth=Normal", "="] {
            assert_eq!(parse(line), FromPanel::Other(line.into()), "{line:?}");
        }
    }

    // Reference 1.1: `HWC#38=36` lights a button, `HWCc#38=137` makes it yellow,
    // `HWCc#38=132` red. 6.2: blink pattern 2 on a lit button is 548.
    #[test]
    fn lights_and_colours_are_written_as_the_reference_gives_them() {
        assert_eq!(hwc_mode(38, Lit::On), "HWC#38=36");
        assert_eq!(hwc_mode(38, Lit::Off), "HWC#38=0");
        assert_eq!(hwc_mode(38, Lit::Dim), "HWC#38=5");
        assert_eq!(hwc_mode(38, Lit::Blink), "HWC#38=548");
        assert_eq!(hwc_color(201, Color::Red), "HWCc#201=132");
        assert_eq!(Color::White.value(), 130);
        assert_eq!(Color::Amber.value(), 136);
        assert_eq!(Color::Blue.value(), 139);
        assert_eq!(Color::Green.value(), 143);
        assert_eq!(Color::Default.value(), 128);
        for lit in [Lit::Off, Lit::Dim, Lit::On, Lit::Blink] {
            assert_eq!(Lit::from_value(lit.value()), lit);
        }
        for color in Color::ALL {
            assert_eq!(Color::from_value(color.value()), Some(color));
        }
    }

    // Reference 1.1 `HWCt#38=|||Camera|1|CAM 1`, and the 7.3 examples
    // `|||Titlestring|1|Text1Label|Text2Label`, `|||Titlestring|1|Text1Label||0`,
    // `|||||Text1Label` and `|||TitleString|1`.
    #[test]
    fn text_is_written_in_the_documented_field_layout() {
        let label = |title: &str, line1: &str, line2: &str| Text {
            title: title.into(),
            label: true,
            line1: line1.into(),
            line2: line2.into(),
            small: false,
        };
        assert_eq!(hwc_text(38, &label("Camera", "CAM 1", "")), "HWCt#38=|||Camera|1|CAM 1");
        assert_eq!(
            hwc_text(1, &label("Titlestring", "Text1Label", "Text2Label")),
            "HWCt#1=|||Titlestring|1|Text1Label|Text2Label"
        );
        assert_eq!(
            hwc_text(1, &Text { small: true, ..label("Titlestring", "Text1Label", "") }),
            "HWCt#1=|||Titlestring|1|Text1Label||0"
        );
        assert_eq!(hwc_text(1, &Text { label: false, ..label("", "Text1Label", "") }), "HWCt#1=|||||Text1Label");
        assert_eq!(hwc_text(1, &label("TitleString", "", "")), "HWCt#1=|||TitleString|1");
        assert_eq!(hwc_text(1, &Text { label: false, ..label("Value", "12", "") }), "HWCt#1=|||Value||12");
        assert_eq!(hwc_text(9, &Text::default()), "HWCt#9=");
    }

    #[test]
    fn text_never_exceeds_the_limit_or_leaves_printable_ascii() {
        let long = Text {
            title: "A title that is far too long for a tile".into(),
            label: true,
            line1: "Stop to position the rig by hand, or turn Record on".into(),
            line2: "Søng | with a pipe and a tab\t".into(),
            small: false,
        };
        let line = hwc_text(16, &long);
        let body = line.strip_prefix("HWCt#16=").unwrap();
        assert!(body.len() <= TEXT_LIMIT, "{} characters", body.len());
        assert!(body.bytes().all(|b| (32..127).contains(&b)), "{body}");
        assert_eq!(body.matches('|').count(), 6, "the pipe in the text must not add a field: {body}");
        let back = parse_text(body);
        assert!(back.title.starts_with("A title") && back.line1.starts_with("Stop to") && back.line2.starts_with("S?ng / with"));
    }

    #[test]
    fn what_we_write_reads_back_the_same() {
        let text = Text { title: "Lease".into(), label: false, line1: "Panel".into(), line2: "Release".into(), small: false };
        match parse_to_panel(&hwc_text(6, &text)) {
            ToPanel::Text { hwcs, value } => {
                assert_eq!(hwcs, vec![6]);
                assert_eq!(parse_text(&value), text);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(parse_to_panel(&hwc_mode(6, Lit::Blink)), ToPanel::Mode { hwcs: vec![6], value: 548 });
        assert_eq!(parse_to_panel(&hwc_color(6, Color::Green)), ToPanel::Color { hwcs: vec![6], value: 143 });
        // Reference 1.1: several components in one command.
        assert_eq!(parse_to_panel("HWC#12,13,14=5"), ToPanel::Mode { hwcs: vec![12, 13, 14], value: 5 });
        assert_eq!(parse_to_panel("HeartBeatTimer=3000"), ToPanel::HeartBeatTimer(3000));
        assert_eq!(parse_to_panel("SleepTimer=0"), ToPanel::SleepTimer(0));
        assert_eq!(parse_to_panel("SleepTimer=2000"), ToPanel::SleepTimer(2000));
        assert_eq!(parse_to_panel("SleepTimer?"), ToPanel::GetSleepTimer);
        assert_eq!(parse_to_panel("WakeUp!"), ToPanel::WakeUp);
        assert_eq!(parse_to_panel("PanelBrightness=8,6"), ToPanel::Brightness("8,6".into()));
        assert_eq!(parse_to_panel("PanelTopology?"), ToPanel::Topology);
        assert_eq!(parse_to_panel("ActivePanel=1"), ToPanel::ActivePanel);
        assert_eq!(parse_to_panel("bogus"), ToPanel::Unknown("bogus".into()));
    }

    #[test]
    fn events_are_written_as_a_panel_writes_them() {
        for line in ["HWC#38=Down", "HWC#35.4=Up", "HWC#42=Enc:-2", "HWC#42=Speed:136", "HWC#25=Abs:768", "HWC#50=Abs:300,700"] {
            match parse(line) {
                FromPanel::Event { hwc, edge, trigger } => assert_eq!(format_event(hwc, edge, &trigger), line),
                other => panic!("{other:?}"),
            }
        }
    }
}
