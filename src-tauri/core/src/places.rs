//! Turns a GPS position into a place name ("Luxembourg, Luxembourg"),
//! without the internet.
//!
//! The app carries a list of the world's 65,000 towns and cities with more
//! than 5,000 inhabitants (from GeoNames, see `data/README.md`). A photo's
//! place is the largest city whose built-up area it is probably in, or else
//! the nearest town within 50 km. That is coarse, but it is what searching
//! "photos from Lisbon" needs.

use std::collections::HashMap;
use std::sync::OnceLock;

const CITIES: &str = include_str!("../data/cities.tsv");
const COUNTRIES: &str = include_str!("../data/countries.tsv");

/// Farther than this from any town, a photo has no place name.
const MAX_DISTANCE_KM: f64 = 50.0;
const EARTH_RADIUS_KM: f64 = 6371.0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    /// For display: "Zürich, Switzerland".
    pub name: String,
    /// For searching: lowercase, without accents: "zuerich zürich switzerland".
    pub search: String,
}

struct City {
    name: &'static str,
    ascii: &'static str,
    country: &'static str,
    latitude: f64,
    longitude: f64,
    population: f64,
}

impl City {
    /// A rough radius of the city's built-up area: 15 km for 2 million
    /// people, 3 km for 100,000. Inside it, the city wins over the suburbs
    /// and districts that the list also has as separate towns.
    fn radius_km(&self) -> f64 {
        self.population.sqrt() / 100.0
    }
}

struct Atlas {
    cities: Vec<City>,
    /// City numbers by one-degree square, to look only at nearby ones.
    squares: HashMap<(i32, i32), Vec<u32>>,
    countries: HashMap<&'static str, &'static str>,
}

fn atlas() -> &'static Atlas {
    static ATLAS: OnceLock<Atlas> = OnceLock::new();
    ATLAS.get_or_init(|| {
        let countries = COUNTRIES
            .lines()
            .filter_map(|line| line.split_once('\t'))
            .collect();
        let mut cities = Vec::new();
        let mut squares: HashMap<(i32, i32), Vec<u32>> = HashMap::new();
        for line in CITIES.lines() {
            let mut fields = line.split('\t');
            let (Some(name), Some(ascii), Some(country)) =
                (fields.next(), fields.next(), fields.next())
            else {
                continue;
            };
            let (Some(latitude), Some(longitude), Some(population)) = (
                fields_number(&mut fields),
                fields_number(&mut fields),
                fields_number(&mut fields),
            ) else {
                continue;
            };
            squares
                .entry(square(latitude, longitude))
                .or_default()
                .push(cities.len() as u32);
            cities.push(City {
                name,
                ascii,
                country,
                latitude,
                longitude,
                population,
            });
        }
        Atlas {
            cities,
            squares,
            countries,
        }
    })
}

fn fields_number<'a>(fields: &mut impl Iterator<Item = &'a str>) -> Option<f64> {
    fields.next()?.parse().ok()
}

fn square(latitude: f64, longitude: f64) -> (i32, i32) {
    (latitude.floor() as i32, longitude.floor() as i32)
}

/// A town found by its name.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Town {
    /// "Metz, France".
    pub name: String,
    pub latitude: f64,
    pub longitude: f64,
}

/// The towns whose name starts with `typed` (case and accents ignored),
/// largest first, at most `limit`.
pub fn find(typed: &str, limit: usize) -> Vec<Town> {
    let typed = crate::clip::fold(typed.trim());
    if typed.chars().count() < 2 {
        return Vec::new();
    }
    let atlas = atlas();
    let mut found: Vec<&City> = atlas
        .cities
        .iter()
        .filter(|city| {
            crate::clip::fold(city.name).starts_with(&typed)
                || (!city.ascii.is_empty() && crate::clip::fold(city.ascii).starts_with(&typed))
        })
        .collect();
    found.sort_by(|a, b| b.population.total_cmp(&a.population));
    found
        .into_iter()
        .take(limit)
        .map(|city| Town {
            name: match atlas.countries.get(city.country) {
                Some(country) => format!("{}, {country}", city.name),
                None => city.name.to_owned(),
            },
            latitude: city.latitude,
            longitude: city.longitude,
        })
        .collect()
}

/// The nearest town to a position, if one is close enough.
pub fn lookup(latitude: f64, longitude: f64) -> Option<Place> {
    if !latitude.is_finite() || !longitude.is_finite() || latitude.abs() > 90.0 {
        return None;
    }
    let atlas = atlas();
    // 50 km is under half a degree of latitude; a degree of longitude
    // shrinks towards the poles, so more squares are needed there.
    let (row, column) = square(latitude, longitude);
    let reach = (0.5 / latitude.to_radians().cos().max(0.01))
        .ceil()
        .min(180.0) as i32;
    let mut nearest: Option<(f64, &City)> = None;
    let mut around: Option<&City> = None;
    for square_row in row - 1..=row + 1 {
        for offset in -reach..=reach {
            // Wrap around the date line.
            let square_column = (column + offset + 180).rem_euclid(360) - 180;
            for &number in atlas
                .squares
                .get(&(square_row, square_column))
                .into_iter()
                .flatten()
            {
                let city = &atlas.cities[number as usize];
                let distance = distance_km(latitude, longitude, city.latitude, city.longitude);
                if distance <= MAX_DISTANCE_KM && nearest.is_none_or(|(least, _)| distance < least)
                {
                    nearest = Some((distance, city));
                }
                if distance <= city.radius_km()
                    && around.is_none_or(|largest| city.population > largest.population)
                {
                    around = Some(city);
                }
            }
        }
    }
    let city = around.or(nearest.map(|(_, city)| city))?;
    let country = atlas
        .countries
        .get(city.country)
        .copied()
        .unwrap_or(city.country);
    let name = format!("{}, {country}", city.name);
    let search = format!("{} {}", city.ascii, name).to_lowercase();
    Some(Place { name, search })
}

/// Great-circle distance (haversine formula).
pub(crate) fn distance_km(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let (lat1, lat2) = (lat1.to_radians(), lat2.to_radians());
    let half_lat = (lat2 - lat1) / 2.0;
    let half_lon = (lon2 - lon1).to_radians() / 2.0;
    let a = half_lat.sin().powi(2) + lat1.cos() * lat2.cos() * half_lon.sin().powi(2);
    2.0 * EARTH_RADIUS_KM * a.sqrt().asin()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_the_nearest_town() {
        let name = |latitude, longitude| lookup(latitude, longitude).map(|place| place.name);
        assert_eq!(
            name(49.6116, 6.1319).as_deref(),
            Some("Luxembourg, Luxembourg")
        );
        assert_eq!(name(48.8584, 2.2945).as_deref(), Some("Paris, France"));
        assert_eq!(
            name(-33.8568, 151.2153).as_deref(),
            Some("Sydney, Australia")
        );
        // Next to the date line, where the squares wrap around.
        assert_eq!(name(-16.43, 179.37).as_deref(), Some("Labasa, Fiji"));
        // The middle of the Atlantic, the South Pole, and nonsense.
        assert_eq!(name(30.0, -40.0), None);
        assert_eq!(name(-90.0, 0.0), None);
        assert_eq!(name(f64::NAN, 0.0), None);
        assert_eq!(name(120.0, 0.0), None);
    }

    #[test]
    fn search_text_has_no_accents() {
        let zurich = lookup(47.3769, 8.5417).unwrap();
        assert_eq!(zurich.name, "Zürich, Switzerland");
        assert!(zurich.search.contains("zuerich") && zurich.search.contains("switzerland"));
    }

    #[test]
    fn distances() {
        // Paris to London is about 344 km.
        let d = distance_km(48.8566, 2.3522, 51.5074, -0.1278);
        assert!((d - 344.0).abs() < 3.0, "{d}");
    }

    #[test]
    fn towns_are_found_by_name() {
        assert_eq!(find("metz", 5)[0].name, "Metz, France");
        assert!(find("Luxem", 5)[0].name.starts_with("Luxembourg"));
        assert!(find("zuri", 5)[0].name.starts_with("Zürich"));
        assert!(find("m", 5).is_empty(), "too short");
    }
}
