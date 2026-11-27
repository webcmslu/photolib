//! Groups photo positions into clusters for the map.
//!
//! The map can't draw 100,000 markers, so photos that would land close
//! together on screen are shown as one marker with a count. "Close" is
//! decided with a grid: the world at the current zoom level is cut into
//! square cells of `CELL_PX` screen pixels, and each cell with photos in it
//! becomes one cluster. Zooming in makes the cells cover less ground, so
//! clusters split up.

use std::collections::HashMap;
use std::f64::consts::PI;

use serde::Serialize;

use crate::db::GeoPoint;

/// Side of a grid cell, in screen pixels. Markers are a bit smaller, so
/// neighbours rarely overlap much.
const CELL_PX: f64 = 80.0;
/// Map tiles are 256 pixels wide; the world is one tile at zoom 0.
const TILE_PX: f64 = 256.0;
const MAX_ZOOM: u8 = 22;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Cluster {
    /// The average position of the photos, where the marker goes.
    pub latitude: f64,
    pub longitude: f64,
    pub count: u32,
    /// The newest photo, shown on the marker.
    pub fingerprint: String,
    /// The rectangle that holds all the photos.
    pub south: f64,
    pub west: f64,
    pub north: f64,
    pub east: f64,
}

/// Clusters `points` for a map at `zoom`. The first point of a cluster
/// becomes its picture, so pass the points newest first.
pub fn cluster(points: &[GeoPoint], zoom: u8) -> Vec<Cluster> {
    struct Sums {
        latitude: f64,
        longitude: f64,
        cluster: Cluster,
    }

    let world_px = TILE_PX * f64::from(1u32 << zoom.min(MAX_ZOOM));
    let mut cells: HashMap<(i64, i64), Sums> = HashMap::new();
    for point in points {
        let (x, y) = web_mercator(point.latitude, point.longitude);
        let cell = (
            (x * world_px / CELL_PX).floor() as i64,
            (y * world_px / CELL_PX).floor() as i64,
        );
        let sums = cells.entry(cell).or_insert_with(|| Sums {
            latitude: 0.0,
            longitude: 0.0,
            cluster: Cluster {
                latitude: 0.0,
                longitude: 0.0,
                count: 0,
                fingerprint: point.fingerprint.clone(),
                south: point.latitude,
                west: point.longitude,
                north: point.latitude,
                east: point.longitude,
            },
        });
        sums.latitude += point.latitude;
        sums.longitude += point.longitude;
        let cluster = &mut sums.cluster;
        cluster.count += 1;
        cluster.south = cluster.south.min(point.latitude);
        cluster.north = cluster.north.max(point.latitude);
        cluster.west = cluster.west.min(point.longitude);
        cluster.east = cluster.east.max(point.longitude);
    }

    let mut clusters: Vec<Cluster> = cells
        .into_values()
        .map(|sums| {
            let count = f64::from(sums.cluster.count);
            Cluster {
                latitude: sums.latitude / count,
                longitude: sums.longitude / count,
                ..sums.cluster
            }
        })
        .collect();
    // A stable order, so the map does not reshuffle markers needlessly.
    clusters.sort_by(|a, b| a.fingerprint.cmp(&b.fingerprint));
    clusters
}

/// Projects a position to map coordinates from 0 to 1: x grows eastwards,
/// y grows southwards, as on the web maps.
fn web_mercator(latitude: f64, longitude: f64) -> (f64, f64) {
    // The projection can't show the poles; this is where web maps end.
    let latitude = latitude.clamp(-85.0511, 85.0511).to_radians();
    let x = (longitude + 180.0) / 360.0;
    let y = (1.0 - (latitude.tan() + 1.0 / latitude.cos()).ln() / PI) / 2.0;
    (x, y)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(name: &str, latitude: f64, longitude: f64) -> GeoPoint {
        GeoPoint {
            latitude,
            longitude,
            fingerprint: name.into(),
        }
    }

    #[test]
    fn projection_matches_the_web_maps() {
        let (x, y) = web_mercator(0.0, 0.0);
        assert!((x - 0.5).abs() < 1e-12 && (y - 0.5).abs() < 1e-12);
        let (x, y) = web_mercator(85.0511, -180.0);
        assert!(x.abs() < 1e-12 && y.abs() < 1e-5);
        // Luxembourg City is in tile 8/132/87.
        let (x, y) = web_mercator(49.6116, 6.1319);
        assert_eq!(((x * 256.0) as u32, (y * 256.0) as u32), (132, 87));
    }

    #[test]
    fn nearby_photos_merge_until_zoomed_in() {
        let points = [
            point("lux-new", 49.6116, 6.1319),
            point("lux-old", 49.6000, 6.1200), // 1.5 km away
            point("paris", 48.8566, 2.3522),
            point("sydney", -33.8688, 151.2093),
        ];

        // Zoomed far out, the two European cities are one marker.
        let world = cluster(&points, 2);
        assert_eq!(world.len(), 2);
        let europe = world.iter().find(|c| c.count == 3).unwrap();
        // The newest photo (first in the input) is its picture.
        assert_eq!(europe.fingerprint, "lux-new");
        assert!((europe.latitude - (49.6116 + 49.6 + 48.8566) / 3.0).abs() < 1e-9);
        assert_eq!((europe.south, europe.north), (48.8566, 49.6116));
        assert_eq!((europe.west, europe.east), (2.3522, 6.1319));

        // Country level: Paris splits off.
        let country = cluster(&points, 7);
        let counts = |clusters: &[Cluster]| {
            let mut counts: Vec<u32> = clusters.iter().map(|c| c.count).collect();
            counts.sort_unstable();
            counts
        };
        assert_eq!(counts(&country), [1, 1, 2]);

        // Street level: every photo by itself.
        assert_eq!(counts(&cluster(&points, 16)), [1, 1, 1, 1]);
        assert!(cluster(&[], 5).is_empty());
    }
}
