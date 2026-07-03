//! Built-in schema templates: ready-made starting points for `init`, and
//! printable format references (`nescio templates --show NAME` dumps one as
//! JSON — the fastest way to see what schema.json and sources.json look
//! like without reading source code).

use std::collections::BTreeMap;

use nescio::prelude::*;

pub struct Template {
    pub name: &'static str,
    pub blurb: &'static str,
    pub build: fn() -> (Schema, Vec<Source>),
}

pub const ALL: &[Template] = &[
    Template {
        name: "real-estate",
        blurb: "villas: coupled price/condition/year_built, sources from land registry to gossip",
        build: real_estate,
    },
    Template {
        name: "osint",
        blurb: "persons of interest: age/role/in_country, sources from court records to informants",
        build: osint,
    },
    Template {
        name: "sensor",
        blurb: "machine monitoring: temperature/vibration/status fused from fast-decaying sensors",
        build: sensor,
    },
];

pub fn get(name: &str) -> Option<(Schema, Vec<Source>)> {
    ALL.iter().find(|t| t.name == name).map(|t| (t.build)())
}

pub fn names() -> String {
    ALL.iter().map(|t| t.name).collect::<Vec<_>>().join(", ")
}

fn source(name: &str, reliability: f64, half_life_days: Option<f64>) -> Source {
    Source {
        name: name.into(),
        reliability,
        half_life_days,
        axiomatic: false,
    }
}

fn axiom(name: &str) -> Source {
    Source {
        name: name.into(),
        reliability: 1.0,
        half_life_days: None,
        axiomatic: true,
    }
}

fn continuous(lo: f64, hi: f64, n_bins: usize) -> Domain {
    Domain::Continuous { lo, hi, n_bins }
}

fn categorical(values: &[&str]) -> Domain {
    Domain::Categorical {
        values: values.iter().map(|v| v.to_string()).collect(),
    }
}

fn gaussian(slot_a: &str, slot_b: &str, centers: &[(&str, f64)], sigma: f64) -> Coupling {
    Coupling {
        slot_a: slot_a.into(),
        slot_b: slot_b.into(),
        compat: Compat::GaussianByCategory {
            centers: centers.iter().map(|(v, c)| (v.to_string(), *c)).collect(),
            sigma,
        },
        name: Some(format!("{slot_a}~{slot_b}")),
    }
}

/// Real-estate objects with coupled slots — the tour template.
fn real_estate() -> (Schema, Vec<Source>) {
    let mut slots = BTreeMap::new();
    slots.insert("price".to_string(), continuous(0.0, 2_000_000.0, 200));
    slots.insert(
        "condition".to_string(),
        categorical(&["renovated", "original", "derelict"]),
    );
    slots.insert("wants_to_sell".to_string(), Domain::boolean());
    slots.insert("year_built".to_string(), continuous(1900.0, 2026.0, 126));
    let couplings = vec![
        gaussian(
            "condition",
            "price",
            &[
                ("renovated", 1_300_000.0),
                ("original", 900_000.0),
                ("derelict", 500_000.0),
            ],
            300_000.0,
        ),
        Coupling {
            slot_a: "year_built".into(),
            slot_b: "condition".into(),
            compat: Compat::StepThreshold {
                threshold: 1980.0,
                below: [("renovated".to_string(), 0.5)].into_iter().collect(),
                above: [("derelict".to_string(), 0.2)].into_iter().collect(),
            },
            name: Some("year_built~condition".into()),
        },
    ];
    let sources = vec![
        axiom("land_registry"),
        axiom("notary"),
        source("broker", 0.85, Some(90.0)),
        source("web_scraper", 0.7, Some(45.0)),
        source("neighbor", 0.4, Some(30.0)),
    ];
    (Schema { slots, couplings }, sources)
}

/// Persons of interest in an investigation. The role~age coupling shows a
/// category without a center ("bystander") staying uninformative.
fn osint() -> (Schema, Vec<Source>) {
    let mut slots = BTreeMap::new();
    slots.insert("age".to_string(), continuous(0.0, 100.0, 100));
    slots.insert(
        "role".to_string(),
        categorical(&["organizer", "financier", "courier", "bystander"]),
    );
    slots.insert("in_country".to_string(), Domain::boolean());
    let couplings = vec![gaussian(
        "role",
        "age",
        &[("organizer", 45.0), ("financier", 52.0), ("courier", 27.0)],
        9.0,
    )];
    let sources = vec![
        axiom("court_record"),
        axiom("passport_office"),
        source("news_wire", 0.75, Some(180.0)),
        source("social_media", 0.6, Some(30.0)),
        source("informant", 0.5, Some(45.0)),
    ];
    (Schema { slots, couplings }, sources)
}

/// Machine monitoring: sensor readings decay in days, not months, and two
/// couplings fuse them into one status belief.
fn sensor() -> (Schema, Vec<Source>) {
    let mut slots = BTreeMap::new();
    slots.insert("temperature".to_string(), continuous(-20.0, 120.0, 140));
    slots.insert("vibration".to_string(), continuous(0.0, 50.0, 100));
    slots.insert(
        "status".to_string(),
        categorical(&["nominal", "degraded", "failing"]),
    );
    let couplings = vec![
        gaussian(
            "status",
            "temperature",
            &[("nominal", 40.0), ("degraded", 70.0), ("failing", 95.0)],
            15.0,
        ),
        gaussian(
            "status",
            "vibration",
            &[("nominal", 4.0), ("degraded", 14.0), ("failing", 30.0)],
            6.0,
        ),
    ];
    let sources = vec![
        axiom("calibration_lab"),
        source("sensor_a", 0.9, Some(2.0)),
        source("sensor_b", 0.85, Some(2.0)),
        source("operator_log", 0.8, Some(30.0)),
    ];
    (Schema { slots, couplings }, sources)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_template_builds_a_working_db() {
        for t in ALL {
            let (schema, sources) = (t.build)();
            let db = Db::in_memory(schema, sources)
                .unwrap_or_else(|e| panic!("template {}: {e}", t.name));
            assert!(!db.schema.slots.is_empty(), "template {}", t.name);
            assert!(!db.sources.is_empty(), "template {}", t.name);
        }
    }
}
