//! `route-tool build IN.gpx OUT.route [TOLERANCE_M]`: the route file the CC600 loads (`tools/send_route.py`).
//! `route-tool info FILE.route`: what it holds.
//! `route-tool sim IN.gpx OUT_DIR [EVERY_S]`: the ride replayed second by second along the GPX (its times, else
//! 20 km/h) through the firmware's screens and navigation (crates/ui), without the device: a PNG of the map and
//! the Essential page every EVERY_S seconds (300), and how far from the route the rider was placed.

mod sim;

use std::process::exit;

use route::{Route, build, parse_gpx};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["build", gpx, out] => build_file(gpx, out, 3.0),
        ["build", gpx, out, tol] => build_file(gpx, out, tol.parse().unwrap_or_else(|_| usage())),
        ["info", file] => info(&std::fs::read(file).unwrap_or_else(|e| fail(file, e))),
        ["sim", gpx, out] => sim::run(gpx, out, 300),
        ["sim", gpx, out, every] => sim::run(gpx, out, every.parse().unwrap_or_else(|_| usage())),
        _ => usage(),
    }
}

fn usage() -> ! {
    eprintln!(
        "usage: route-tool build IN.gpx OUT.route [TOLERANCE_M]\n       route-tool info FILE.route\n       \
         route-tool sim IN.gpx OUT_DIR [EVERY_S]"
    );
    exit(2)
}

pub(crate) fn fail(what: &str, e: impl std::fmt::Display) -> ! {
    eprintln!("{what}: {e}");
    exit(1)
}

fn build_file(gpx: &str, out: &str, tolerance_m: f64) {
    let text = std::fs::read_to_string(gpx).unwrap_or_else(|e| fail(gpx, e));
    let g = parse_gpx(&text).unwrap_or_else(|e| fail(gpx, format!("{e:?}")));
    let file = build(&g, tolerance_m);
    std::fs::write(out, &file).unwrap_or_else(|e| fail(out, e));
    println!("{} GPX points -> {out}", g.points.len());
    info(&file);
}

fn info(file: &[u8]) {
    let r = Route::parse(file).unwrap_or_else(|e| fail("route", format!("{e:?}")));
    let i = r.info;
    println!("name      {}", r.name());
    println!("points    {} ({} bytes, {:.1} bytes a point)", i.count, file.len(), file.len() as f64 / i.count as f64);
    println!("length    {:.1} km", i.length_dm as f64 / 10_000.0);
    println!("climb     +{} m / -{} m, {:.0} to {:.0} m", i.ascent_m, i.descent_m, i.min_ele_dm as f64 / 10.0, i.max_ele_dm as f64 / 10.0);
    let [x0, y0, x1, y1] = i.bbox;
    println!("extent    {:.1} x {:.1} km", (x1 - x0) as f64 / 10_000.0, (y1 - y0) as f64 / 10_000.0);
}
