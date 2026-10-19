//! Routes built from synthetic GPX files and read back as the device does.

use route::{Error, Route, build, parse_gpx};

/// A GPX of `points` (lat, lon, ele), as Komoot writes them.
fn gpx(name: &str, points: &[(f64, f64, f64)]) -> String {
    let mut s = format!("<?xml version='1.0'?><gpx><trk><name>{name}</name><trkseg>\n");
    for (lat, lon, ele) in points {
        s += &format!("<trkpt lat=\"{lat:.6}\" lon=\"{lon:.6}\">\n<ele>{ele:.1}</ele>\n<time>2026-10-18T09:40:26.5Z</time>\n</trkpt>\n");
    }
    s + "</trkseg></trk></gpx>"
}

const LAT: f64 = 45.0;
const LON: f64 = 3.0;
/// Degrees for 1 km north, and east at LAT.
const KM_LAT: f64 = 1.0 / 111.319_49;

fn km_lon() -> f64 {
    KM_LAT / LAT.to_radians().cos()
}

/// A 1 km square, clockwise from the south west corner (north first), a point every 10 m, climbing 50 m on the
/// north side.
fn square() -> Vec<(f64, f64, f64)> {
    let mut p = vec![];
    let corners = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0), (0.0, 0.0)];
    for w in corners.windows(2) {
        let ((n0, e0), (n1, e1)) = (w[0], w[1]);
        for i in 0..100 {
            let f = i as f64 / 100.0;
            let (n, e) = (n0 + (n1 - n0) * f, e0 + (e1 - e0) * f);
            let ele = if n0 == 1.0 && n1 == 1.0 { 100.0 + 50.0 * f } else if n0 == 1.0 { 150.0 } else { 100.0 };
            p.push((LAT + n * KM_LAT, LON + e * km_lon(), ele));
        }
    }
    p.push((LAT, LON, 100.0));
    p
}

#[test]
fn a_square_is_simplified_and_read_back() {
    let g = parse_gpx(&gpx("Carré &amp; côte", &square())).unwrap();
    assert_eq!(g.name, "Carré & côte");
    assert_eq!(g.points.len(), 401);
    // 2026-10-18 09:40:26.5 UTC
    assert_eq!(g.times[0], Some(1_792_316_426.5));
    let file = build(&g, 3.0);
    let r = Route::parse(&file).unwrap();
    assert_eq!(r.name(), "Carré & côte");
    // straight sides: the corners only, and the points where the elevation changes slope stay out
    assert!(r.info.count <= 8, "{} points", r.info.count);
    assert!((r.info.length_dm as i64 - 40_000).abs() < 40, "{} dm", r.info.length_dm);
    assert_eq!(r.info.ascent_m, 50);
    assert!(file.len() < 200, "{} bytes", file.len());
    let pts: Vec<_> = r.points().collect();
    assert_eq!(pts.len() as u32, r.info.count);
    assert_eq!(pts.last().unwrap().along_dm, r.info.length_dm);
}

#[test]
fn locating_the_rider() {
    let r_file = build(&parse_gpx(&gpx("c", &square())).unwrap(), 3.0);
    let r = Route::parse(&r_file).unwrap();
    // 20 m outside the east side (the third: west side north, north side east, then south), half way: 2.5 km
    let (x, y) = r.project(
        ((LAT + 0.5 * KM_LAT) * 1e7).round() as i32,
        ((LON + 1.02 * km_lon()) * 1e7).round() as i32,
    );
    let p = r.locate(x, y, None);
    assert!((p.along_dm as i64 - 25_000).abs() < 30, "{p:?}");
    assert!((p.off_dm as i64 - 200).abs() < 5, "{p:?}");
}

#[test]
fn an_out_and_back_keeps_its_direction() {
    // 2 km north and back on the same road
    let mut pts = vec![];
    for i in 0..=200 {
        pts.push((LAT + i as f64 * 0.01 * KM_LAT, LON, 100.0));
    }
    for i in (0..200).rev() {
        pts.push((LAT + i as f64 * 0.01 * KM_LAT, LON + 0.00001, 100.0));
    }
    let file = build(&parse_gpx(&gpx("aller-retour", &pts)).unwrap(), 3.0);
    let r = Route::parse(&file).unwrap();
    let at_500m = r.project(((LAT + 0.5 * KM_LAT) * 1e7) as i32, (LON * 1e7) as i32);
    // going out: about 500 m along
    let out = r.locate(at_500m.0, at_500m.1, None);
    assert!(out.along_dm < 6_000, "{out:?}");
    // coming back past the same place, 3.4 km along before: the back pass (3.5 km)
    let mut last = r.locate(at_500m.0, at_500m.1, None);
    last.along_dm = 34_000;
    let back = r.locate(at_500m.0, at_500m.1, Some(last));
    assert!((back.along_dm as i64 - 35_000).abs() < 100, "{back:?}");
}

#[test]
fn damaged_files_are_refused() {
    let file = build(&parse_gpx(&gpx("c", &square())).unwrap(), 3.0);
    let mut bad = file.clone();
    bad[40] ^= 1;
    assert_eq!(Route::parse(&bad).err(), Some(Error::Corrupt));
    assert_eq!(Route::parse(&file[..file.len() - 5]).err(), Some(Error::Corrupt));
    assert_eq!(Route::parse(b"GPX!").err(), Some(Error::Format));
}

#[test]
fn gpx_variants() {
    // route points, single quotes, attributes the other way round, self-closing points without elevation
    let text = "<gpx><rte><name>R</name><rtept lon='3.01' lat='45.01'/>\n<rtept  lat='45.02' lon='3.02'/></rte></gpx>";
    let g = parse_gpx(text).unwrap();
    assert_eq!(g.name, "R");
    assert_eq!(g.points, vec![(45.01, 3.01, None), (45.02, 3.02, None)]);
    assert_eq!(g.times, vec![None, None]);
    assert!(Route::parse(&build(&g, 3.0)).is_ok());
    assert_eq!(parse_gpx("<gpx></gpx>").err(), Some(route::GpxError::NoPoints));
    assert_eq!(
        parse_gpx("<gpx><trkpt lat='a' lon='1'></trkpt><trkpt lat='1' lon='1'/></gpx>").err(),
        Some(route::GpxError::BadPoint)
    );
}
