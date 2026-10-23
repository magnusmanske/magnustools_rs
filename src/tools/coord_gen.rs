//! `coord_gen.php`: German Wikipedia's `{{Coordinate}}` template from
//! degrees, minutes and seconds, at each precision the input allows.

use crate::web::{AppError, Params, render};
use askama::Template;
use axum::response::Html;
use serde::Deserialize;

/// `type=` values, with what they are for.
const TYPES: &[(&str, &str)] = &[
    ("landmark", "Landmark, building, anything else"),
    ("city", "City (give the population)"),
    ("mountain", "Mountain, hill, range (give the elevation)"),
    ("waterbody", "River, lake, canal, waterfall"),
    ("isle", "Island, archipelago"),
    ("airport", "Airport, airbase"),
    ("adm2nd", "District, municipal association"),
    ("adm1st", "State, canton"),
    ("state", "Dependent territory"),
    ("country", "Country"),
];

/// `template_name` values: where the coordinate goes, and the parameters that says.
const PLACEMENTS: &[(&str, &str, &str)] = &[
    ("article", "Whole article", "article=|"),
    ("text", "In the text", "text=|name=…|"),
    ("both", "Both", "text=|article=|"),
];

/// The parameter names of the original (German) form, so old links keep working.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Request {
    breite_grad: String,
    breite_minute: String,
    breite_sekunde: String,
    breite_dir: String,
    laenge_grad: String,
    laenge_minute: String,
    laenge_sekunde: String,
    laenge_dir: String,
    data_type: String,
    data_size: String,
    template_name: String,
    doit: Option<String>,
}

/// One axis of a coordinate: degrees, and optionally minutes and seconds.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Angle {
    degrees: u32,
    minutes: Option<u32>,
    seconds: Option<u32>,
    /// `N`, `S`, `E` or `W`.
    direction: char,
}

impl Angle {
    fn parse(degrees: &str, minutes: &str, seconds: &str, direction: &str, max: u32) -> Result<Self, String> {
        let number = |s: &str, limit: u32| -> Result<Option<u32>, String> {
            match s.trim() {
                "" => Ok(None),
                s => s.parse().ok().filter(|n| *n < limit).map(Some).ok_or_else(|| format!("“{s}” is out of range.")),
            }
        };
        let degrees = number(degrees, max + 1)?.ok_or("Degrees are missing.")?;
        let minutes = number(minutes, 60)?;
        let seconds = minutes.and(number(seconds, 60)?);
        if degrees == max && minutes.unwrap_or_default() + seconds.unwrap_or_default() > 0 {
            return Err(format!("{degrees}° is the most there is."));
        }
        let directions = if max == 90 { ['N', 'S'] } else { ['E', 'W'] };
        let direction = direction.trim().chars().next().filter(|d| directions.contains(d)).unwrap_or(directions[0]);
        Ok(Self { degrees, minutes, seconds, direction })
    }

    /// At most `precision` parts: `52/31/12/N`; missing parts empty.
    fn parameter(&self, precision: usize) -> String {
        let part = |n: Option<u32>| n.map(|n| n.to_string()).unwrap_or_default();
        let parts = [Some(self.degrees), self.minutes, self.seconds];
        let parts: Vec<String> = parts.iter().enumerate().map(|(i, n)| part(n.filter(|_| i < precision))).collect();
        format!("{}/{}", parts.join("/"), self.direction)
    }

    /// As German Wikipedia shows it: `52° 31′ 12″ N` (East is `O`).
    fn display(&self, precision: usize) -> String {
        let mut text = format!("{}°", self.degrees);
        for (n, unit) in [(self.minutes, '′'), (self.seconds, '″')].into_iter().take(precision - 1) {
            text.push_str(&format!(" {}{unit}", n.unwrap_or_default()));
        }
        let direction = if self.direction == 'E' { 'O' } else { self.direction };
        format!("{text} {direction}")
    }

    /// How many parts it has: 1 (degrees) to 3 (seconds).
    fn precision(&self) -> usize {
        1 + usize::from(self.minutes.is_some()) + usize::from(self.seconds.is_some())
    }
}

struct Row {
    precision: &'static str,
    wikitext: String,
    preview: String,
}

#[derive(Template)]
#[template(path = "tools/coord_gen.html")]
struct Page<'a> {
    title: &'static str,
    request: &'a Request,
    types: &'static [(&'static str, &'static str)],
    placements: &'static [(&'static str, &'static str, &'static str)],
    /// The chosen placement's name.
    placement: &'static str,
    rows: Vec<Row>,
}

pub async fn handler(Params(request): Params<Request>) -> Result<Html<String>, AppError> {
    let rows = match request.doit {
        Some(_) => rows(&request).map_err(AppError::BadRequest)?,
        None => vec![],
    };
    let placement = placement(&request.template_name).0;
    render(&Page {
        title: "Coordinate generator",
        request: &request,
        types: TYPES,
        placements: PLACEMENTS,
        placement,
        rows,
    })
}

fn rows(r: &Request) -> Result<Vec<Row>, String> {
    let latitude = Angle::parse(&r.breite_grad, &r.breite_minute, &r.breite_sekunde, &r.breite_dir, 90)?;
    let longitude = Angle::parse(&r.laenge_grad, &r.laenge_minute, &r.laenge_sekunde, &r.laenge_dir, 180)?;
    let kind = TYPES.iter().find(|(t, _)| *t == r.data_type).map_or("landmark", |(t, _)| t);
    let size = r.data_size.trim();
    if !size.chars().all(|c| c.is_ascii_digit() || c == '.' || c == '-') {
        return Err(format!("“{size}” is not a number."));
    }
    let extra = match (kind, size.is_empty()) {
        ("city", false) => format!("pop={size}|"),
        ("mountain", false) => format!("elevation={size}|"),
        _ => String::new(),
    };
    let placement = placement(&r.template_name).2;
    let precision = latitude.precision().min(longitude.precision());
    let rows = [(3, "Degrees, minutes, seconds"), (2, "Degrees, minutes"), (1, "Degrees")]
        .into_iter()
        .filter(|(p, _)| *p <= precision)
        .map(|(p, label)| Row {
            precision: label,
            wikitext: format!(
                "{{{{Coordinate|{placement}NS={}|EW={}|type={kind}|{extra}region=…}}}}",
                latitude.parameter(p),
                longitude.parameter(p)
            ),
            preview: format!("{}, {}", latitude.display(p), longitude.display(p)),
        })
        .collect();
    Ok(rows)
}

/// The placement by name; the original form sent the parameters themselves. Defaults to the first.
fn placement(value: &str) -> &'static (&'static str, &'static str, &'static str) {
    PLACEMENTS.iter().find(|(name, _, params)| *name == value || *params == value).unwrap_or(&PLACEMENTS[0])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(lat: [&str; 4], lon: [&str; 4]) -> Request {
        let s = |v: &str| v.to_string();
        Request {
            breite_grad: s(lat[0]),
            breite_minute: s(lat[1]),
            breite_sekunde: s(lat[2]),
            breite_dir: s(lat[3]),
            laenge_grad: s(lon[0]),
            laenge_minute: s(lon[1]),
            laenge_sekunde: s(lon[2]),
            laenge_dir: s(lon[3]),
            data_type: s("city"),
            data_size: s("3700000"),
            template_name: s("article"),
            doit: Some(s("OK")),
        }
    }

    #[test]
    fn all_precisions() {
        let rows = rows(&request(["52", "31", "12", "N"], ["13", "24", "36", "E"])).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(
            rows[0].wikitext,
            "{{Coordinate|article=|NS=52/31/12/N|EW=13/24/36/E|type=city|pop=3700000|region=…}}"
        );
        assert_eq!(rows[0].preview, "52° 31′ 12″ N, 13° 24′ 36″ O");
        assert_eq!(rows[2].wikitext, "{{Coordinate|article=|NS=52///N|EW=13///E|type=city|pop=3700000|region=…}}");
        assert_eq!(rows[2].preview, "52° N, 13° O");
    }

    #[test]
    fn only_what_both_axes_have() {
        let rows = rows(&request(["33", "52", "", "S"], ["151", "12", "30", "E"])).unwrap();
        assert_eq!(rows.iter().map(|r| r.precision).collect::<Vec<_>>(), ["Degrees, minutes", "Degrees"]);
        assert_eq!(rows[0].wikitext, "{{Coordinate|article=|NS=33/52//S|EW=151/12//E|type=city|pop=3700000|region=…}}");
    }

    #[test]
    fn bad_input() {
        assert!(rows(&request(["91", "", "", "N"], ["0", "", "", "E"])).is_err());
        assert!(rows(&request(["10", "60", "", "N"], ["0", "", "", "E"])).is_err());
        assert!(rows(&request(["90", "30", "", "N"], ["0", "", "", "E"])).is_err());
        assert!(rows(&request(["90", "0", "30", "N"], ["0", "", "", "E"])).is_err());
        assert!(rows(&request(["90", "0", "0", "N"], ["180", "", "", "W"])).is_ok());
        assert!(rows(&request(["", "", "", "N"], ["0", "", "", "E"])).is_err());
        let mut r = request(["1", "", "", "N"], ["2", "", "", "E"]);
        r.data_size = "1|evil=1".into();
        assert!(rows(&r).is_err());
    }
}
