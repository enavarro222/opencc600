//! `route-tool map IN.gpx OSM.json[,MORE.json] OUT.map [CORRIDOR_M]`: the background map of the route built from
//! the same GPX (`build`, 3 m), from OSM ways (Overpass JSON, `out geom`: roads, tracks, paths, waterways, water,
//! forests) and contour lines in the same form (tags `contour=elevation`, `ele`).

use route::map::class;
use route::{MapFeature, build, build_map, parse_gpx};
use serde_json::Value;

use crate::fail;

/// Class of an OSM way from its tags; `None`: not drawn.
fn classify(tags: &Value) -> Option<u8> {
    let tag = |k: &str| tags.get(k).and_then(Value::as_str);
    if let Some(h) = tag("highway") {
        return match h.trim_end_matches("_link") {
            "motorway" | "trunk" | "primary" | "secondary" => Some(class::MAJOR),
            "tertiary" | "unclassified" | "residential" | "living_street" | "road" => Some(class::MINOR),
            "track" => Some(class::TRACK),
            "path" | "footway" | "cycleway" | "bridleway" | "steps" | "pedestrian" => Some(class::PATH),
            "service" => Some(class::SERVICE),
            _ => None,
        };
    }
    if tag("contour") == Some("elevation") {
        let ele: i32 = tag("ele")?.parse().ok()?;
        return Some(if ele % 50 == 0 { class::CONTOUR_MAJOR } else { class::CONTOUR });
    }
    match (tag("waterway"), tag("natural"), tag("landuse")) {
        (Some("river" | "canal"), _, _) => Some(class::RIVER),
        (Some("stream" | "ditch" | "drain"), _, _) => Some(class::STREAM),
        (_, Some("water"), _) => Some(class::WATER),
        (_, Some("wood"), _) | (_, _, Some("forest")) => Some(class::FOREST),
        (_, _, Some("residential")) => Some(class::RESIDENTIAL),
        _ => None,
    }
}

pub fn run(gpx_path: &str, osm_path: &str, out: &str, corridor_m: f64) {
    let text = std::fs::read_to_string(gpx_path).unwrap_or_else(|e| fail(gpx_path, e));
    let gpx = parse_gpx(&text).unwrap_or_else(|e| fail(gpx_path, format!("{e:?}")));
    let route_file = build(&gpx, 3.0);
    let files: Vec<Value> = osm_path
        .split(',')
        .map(|p| serde_json::from_slice(&std::fs::read(p).unwrap_or_else(|e| fail(p, e))).unwrap_or_else(|e| fail(p, e)))
        .collect();
    let e7 = |v: &Value| v.as_f64().map(|d| (d * 1e7).round() as i32);
    let mut features = Vec::new();
    for el in files.iter().flat_map(|f| f["elements"].as_array().into_iter().flatten()) {
        let Some(c) = classify(&el["tags"]) else { continue };
        // a way, or the outer rings of a multipolygon (inner rings, the holes, are left out)
        let rings: Vec<&Value> = match el["type"].as_str() {
            Some("way") => vec![&el["geometry"]],
            _ => el["members"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|m| m["role"] == "outer")
                .map(|m| &m["geometry"])
                .collect(),
        };
        for g in rings {
            // runs of nodes with coordinates (null: out of the query's box)
            let mut run = Vec::new();
            for n in g.as_array().into_iter().flatten().chain([&Value::Null]) {
                match (e7(&n["lat"]), e7(&n["lon"])) {
                    (Some(lat), Some(lon)) => run.push((lat, lon)),
                    _ if run.len() > 1 => features.push(MapFeature { class: c, points: std::mem::take(&mut run) }),
                    _ => run.clear(),
                }
            }
        }
    }
    let (file, stats) = build_map(&route_file, &features, corridor_m);
    std::fs::write(out, &file).unwrap_or_else(|e| fail(out, e));
    const NAMES: [&str; class::COUNT] = [
        "forest", "water", "residential", "contour", "contour50", "stream", "river", "path", "track", "service",
        "minor", "major",
    ];
    println!("{} features -> {out}, {} bytes", features.len(), file.len());
    for (level, name) in ["detail", "overview"].iter().enumerate() {
        let total: usize = stats.bytes[level].iter().sum();
        let points: usize = stats.points[level].iter().sum();
        let by: Vec<String> = (0..class::COUNT)
            .filter(|&c| stats.bytes[level][c] > 0)
            .map(|c| format!("{} {:.1}", NAMES[c], stats.bytes[level][c] as f64 / 1000.0))
            .collect();
        println!(
            "{name:8} {} cells, {points} points, {:.1} KB: {}",
            stats.cells[level],
            total as f64 / 1000.0,
            by.join(", ")
        );
    }
}
