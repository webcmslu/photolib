# Bundled data

`cities.tsv` and `countries.tsv` come from [GeoNames](https://www.geonames.org/)
(`cities5000.zip` and `countryInfo.txt` from
<https://download.geonames.org/export/dump/>), licensed under
[Creative Commons Attribution 4.0](https://creativecommons.org/licenses/by/4.0/).

They are trimmed to the columns the app uses:

- `cities.tsv`: name, ASCII name (empty when identical), country code,
  latitude, longitude, population. Every town with more than 5,000 inhabitants; districts of
  a city (GeoNames feature code `PPLX`) are left out.
- `countries.tsv`: country code, country name.

The app uses them to name the place where a photo was taken (`src/places.rs`).
