use anyhow::{anyhow, Result};
use chrono::{NaiveDate, Utc};
use csv::Reader;
use media_sync_models::{MediaType, Rating, RatingSource, WatchHistory, WatchlistItem};
use std::fs::File;
use std::path::Path;
use tracing::{self, debug};

/// Parse IMDB watchlist CSV
pub fn parse_watchlist_csv<P: AsRef<Path>>(path: P) -> Result<Vec<WatchlistItem>> {
    let file = File::open(path)?;
    let mut reader = Reader::from_reader(file);
    let mut watchlist = Vec::new();

    // Read header
    let headers = reader.headers()?.clone();
    let header_map: std::collections::HashMap<String, usize> = headers
        .iter()
        .enumerate()
        .map(|(i, h)| (h.to_string(), i))
        .collect();

    // Log available columns for debugging
    let available_columns: Vec<String> = headers.iter().map(|h| h.to_string()).collect();
    tracing::debug!("Available CSV columns: {:?}", available_columns);

    // Required columns (Created is optional - will use current date if missing)
    let required = ["Title", "Year", "Const", "Title Type"];
    for col in &required {
        if !header_map.contains_key(*col) {
            return Err(anyhow!("Missing required column: {}. Available columns: {:?}", col, available_columns));
        }
    }
    
    // Check if Created column exists (optional)
    let has_created_column = header_map.contains_key("Created");
    if !has_created_column {
        tracing::warn!("CSV missing 'Created' column - will use current date for date_added");
    }

    // Parse rows
    let mut row_count = 0;
    for result in reader.records() {
        let record = result?;
        row_count += 1;
        
        let title = record.get(header_map["Title"]).unwrap_or("").to_string();
        let year_str = record.get(header_map["Year"]).unwrap_or("");
        let year = year_str.parse::<u32>().ok();
        let imdb_id = record.get(header_map["Const"]).unwrap_or("").to_string();
        let created_str = if has_created_column {
            record.get(header_map["Created"]).unwrap_or("").to_string()
        } else {
            String::new()
        };
        let title_type = record.get(header_map["Title Type"]).unwrap_or("").to_string();

        // Debug first few rows
        if row_count <= 3 {
            tracing::debug!(
                row = row_count,
                imdb_id = %imdb_id,
                title = %title,
                year = ?year,
                title_type = %title_type,
                created = %created_str,
                has_created_column = has_created_column,
                "Parsing watchlist CSV row"
            );
        }

        if imdb_id.is_empty() {
            tracing::debug!(row = row_count, "Skipping row with empty IMDB ID");
            continue;
        }

        // Parse date: YYYY-MM-DD -> DateTime<Utc>
        // If Created column is missing, use current date as fallback
        let date_added = if has_created_column && !created_str.is_empty() {
            chrono::NaiveDate::parse_from_str(&created_str, "%Y-%m-%d")
                .map_err(|e| anyhow!("Failed to parse date '{}': {}", created_str, e))?
                .and_hms_opt(0, 0, 0)
                .ok_or_else(|| anyhow!("Failed to create time from date '{}'", created_str))?
                .and_local_timezone(Utc)
                .earliest()
                .ok_or_else(|| anyhow!("Failed to convert date '{}' to UTC", created_str))?
        } else {
            // Use current date/time as fallback if Created column is missing
            Utc::now()
        };

        // Map Title Type to MediaType
        let media_type = match title_type.as_str() {
            "TV Series" | "TV Mini Series" => MediaType::Show,
            "TV Episode" => {
                // For episodes, we don't have season/episode numbers in CSV
                // Use placeholder values - these should be updated from Trakt API if needed
                MediaType::Episode {
                    season: 0,
                    episode: 0,
                }
            }
            "Movie" | "TV Special" | "TV Movie" | "TV Short" | "Video" => MediaType::Movie,
            _ => {
                tracing::debug!(
                    row = row_count,
                    title_type = %title_type,
                    "Skipping row with unknown title type"
                );
                continue; // Skip unknown types
            }
        };

        watchlist.push(WatchlistItem {
            imdb_id: imdb_id.clone(),
            ids: None,
            title: title.clone(),
            year,
            media_type,
            date_added,
            source: "imdb".to_string(),
            status: Some(media_sync_models::NormalizedStatus::Watchlist), // IMDB watchlist items are always "Watchlist" status
        });
        
        // Debug first few items added
        if watchlist.len() <= 3 {
            tracing::debug!(
                item_count = watchlist.len(),
                imdb_id = %imdb_id,
                title = %title,
                "Added watchlist item"
            );
        }
    }

    tracing::info!("Parsed {} total rows, {} valid watchlist items", row_count, watchlist.len());
    Ok(watchlist)
}

/// Parse IMDB ratings CSV
pub fn parse_ratings_csv<P: AsRef<Path>>(path: P) -> Result<Vec<Rating>> {
    let file = File::open(path)?;
    let mut reader = Reader::from_reader(file);
    let mut ratings = Vec::new();

    // Read header
    let headers = reader.headers()?.clone();
    let header_map: std::collections::HashMap<String, usize> = headers
        .iter()
        .enumerate()
        .map(|(i, h)| (h.to_string(), i))
        .collect();

    // Required columns
    let required = ["Title", "Year", "Your Rating", "Const", "Date Rated", "Title Type"];
    for col in &required {
        if !header_map.contains_key(*col) {
            return Err(anyhow!("Missing required column: {}", col));
        }
    }

    // Parse rows
    let mut row_count = 0;
    for result in reader.records() {
        let record = result?;
        row_count += 1;
        
        let title = record.get(header_map["Title"]).unwrap_or("").to_string();
        let year_str = record.get(header_map["Year"]).unwrap_or("");
        let year = year_str.parse::<u32>().ok();
        let rating_str = record.get(header_map["Your Rating"]).unwrap_or("");
        let imdb_id = record.get(header_map["Const"]).unwrap_or("").to_string();
        let date_rated_str = record.get(header_map["Date Rated"]).unwrap_or("");
        let title_type = record.get(header_map["Title Type"]).unwrap_or("").to_string();

        // Debug first few rows
        if row_count <= 3 {
            tracing::debug!(
                row = row_count,
                imdb_id = %imdb_id,
                title = %title,
                rating = %rating_str,
                title_type = %title_type,
                date_rated = %date_rated_str,
                "Parsing ratings CSV row"
            );
        }

        if imdb_id.is_empty() {
            tracing::debug!(row = row_count, "Skipping row with empty IMDB ID");
            continue;
        }

        // Parse rating: IMDB CSV uses integer ratings 1-10 (matching Python: int(rating))
        let rating = rating_str.parse::<u8>()
            .map_err(|e| anyhow!("Failed to parse rating '{}': {}", rating_str, e))?;

        // Parse date: YYYY-MM-DD -> DateTime<Utc>
        // Use NaiveDate first, then convert to DateTime<Utc>
        let date_added = NaiveDate::parse_from_str(date_rated_str, "%Y-%m-%d")
            .map_err(|e| anyhow!("Failed to parse date '{}': {}", date_rated_str, e))?
            .and_hms_opt(0, 0, 0)
            .ok_or_else(|| anyhow!("Failed to create time from date '{}'", date_rated_str))?
            .and_local_timezone(Utc)
            .earliest()
            .ok_or_else(|| anyhow!("Failed to convert date '{}' to UTC", date_rated_str))?;

        // Map Title Type to MediaType
        let media_type = match title_type.as_str() {
            "TV Series" | "TV Mini Series" => MediaType::Show,
            "TV Episode" => {
                MediaType::Episode {
                    season: 0,
                    episode: 0,
                }
            }
            "Movie" | "TV Special" | "TV Movie" | "TV Short" | "Video" => MediaType::Movie,
            _ => continue,
        };

        ratings.push(Rating {
            imdb_id: imdb_id.clone(),
            ids: None,
            rating,
            date_added,
            media_type,
            source: media_sync_models::RatingSource::Imdb,
        });
        
        // Debug first few items added
        if ratings.len() <= 3 {
            tracing::debug!(
                item_count = ratings.len(),
                imdb_id = %imdb_id,
                rating = rating,
                "Added rating"
            );
        }
    }

    tracing::info!("Parsed {} total rows, {} valid ratings", row_count, ratings.len());
    Ok(ratings)
}

/// Parse IMDB check-ins CSV (watch history)
pub fn parse_checkins_csv<P: AsRef<Path>>(path: P) -> Result<Vec<WatchHistory>> {
    let file = File::open(path)?;
    let mut reader = Reader::from_reader(file);
    let mut history = Vec::new();

    // Read header
    let headers = reader.headers()?.clone();
    let header_map: std::collections::HashMap<String, usize> = headers
        .iter()
        .enumerate()
        .map(|(i, h)| (h.to_string(), i))
        .collect();

    // Log available columns for debugging
    let available_columns: Vec<String> = headers.iter().map(|h| h.to_string()).collect();
    tracing::debug!("Available check-ins CSV columns: {:?}", available_columns);

    // Required columns (Created is optional for check-ins - will skip items without date)
    let required = ["Title", "Year", "Const", "Title Type"];
    for col in &required {
        if !header_map.contains_key(*col) {
            return Err(anyhow!("Missing required column: {}. Available columns: {:?}", col, available_columns));
        }
    }
    
    // Check if Created column exists (optional)
    let has_created_column = header_map.contains_key("Created");
    if !has_created_column {
        tracing::warn!("CSV missing 'Created' column - will skip check-ins without watch date");
    }

    // Parse rows
    let mut row_count = 0;
    for result in reader.records() {
        let record = result?;
        row_count += 1;
        let year_str = record.get(header_map["Year"]).unwrap_or("");
        let imdb_id = record.get(header_map["Const"]).unwrap_or("").to_string();
        let created_str = if has_created_column {
            record.get(header_map["Created"]).unwrap_or("").to_string()
        } else {
            String::new()
        };
        let title_type = record.get(header_map["Title Type"]).unwrap_or("").to_string();
        let title = record.get(header_map["Title"]).unwrap_or("").to_string();

        // Debug: Log first few rows
        if row_count <= 5 {
            debug!(
                row = row_count,
                imdb_id = %imdb_id,
                title = %title,
                year = %year_str,
                title_type = %title_type,
                created = %created_str,
                has_created_column = has_created_column,
                "Parsing check-ins CSV row"
            );
        }

        if imdb_id.is_empty() {
            tracing::debug!(row = row_count, "Skipping row with empty IMDB ID");
            continue;
        }

        // Skip rows without Created date (we need the watch date for check-ins)
        if !has_created_column || created_str.is_empty() {
            tracing::debug!(row = row_count, "Skipping check-in without Created date");
            continue;
        }

        // Parse date: YYYY-MM-DD -> DateTime<Utc>
        let watched_at = NaiveDate::parse_from_str(&created_str, "%Y-%m-%d")
            .map_err(|e| anyhow!("Failed to parse date '{}': {}", created_str, e))?
            .and_hms_opt(0, 0, 0)
            .ok_or_else(|| anyhow!("Failed to create time from date '{}'", created_str))?
            .and_local_timezone(Utc)
            .earliest()
            .ok_or_else(|| anyhow!("Failed to convert date '{}' to UTC", created_str))?;

        // Map Title Type to MediaType
        let media_type = match title_type.as_str() {
            "TV Series" | "TV Mini Series" => MediaType::Show,
            "TV Episode" => {
                MediaType::Episode {
                    season: 0,
                    episode: 0,
                }
            }
            "Movie" | "TV Special" | "TV Movie" | "TV Short" | "Video" => MediaType::Movie,
            _ => continue,
        };

        // Parse year from year_str
        let year = year_str.parse::<u32>().ok();

        // Debug: Log first few items
        if history.len() < 5 {
            debug!(
                item_count = history.len() + 1,
                imdb_id = %imdb_id,
                title = %title,
                watched_at = %watched_at,
                media_type = ?&media_type,
                "Adding watch history item from CSV"
            );
        }

        history.push(WatchHistory {
            imdb_id: imdb_id.clone(),
            ids: None,
            title: if title.is_empty() { None } else { Some(title) },
            year,
            watched_at,
            media_type,
            source: "imdb".to_string(),
        });
    }

    tracing::info!("Parsed {} total rows, {} valid watch history items from check-ins CSV", row_count, history.len());

    Ok(history)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn create_watchlist_csv() -> NamedTempFile {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            "Position,Const,Created,Modified,Description,Title,URL,Title Type,IMDb Rating,Runtime (mins),Year,Genres,Num Votes,Release Date,Directors"
        ).unwrap();
        writeln!(
            file,
            "1,tt0111161,2020-01-01,2020-01-01,,The Shawshank Redemption,https://www.imdb.com/title/tt0111161/,Movie,9.3,142,1994,Drama,2500000,1994-09-23,Frank Darabont"
        ).unwrap();
        writeln!(
            file,
            "2,tt0944947,2020-01-02,2020-01-02,,Game of Thrones,https://www.imdb.com/title/tt0944947/,TV Series,9.2,57,2011,Action Drama Fantasy,2000000,2011-04-17,"
        ).unwrap();
        file
    }

    fn create_ratings_csv() -> NamedTempFile {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            "Const,Your Rating,Date Rated,Title,URL,Title Type,IMDb Rating,Runtime (mins),Year,Genres,Num Votes,Release Date,Directors"
        ).unwrap();
        writeln!(
            file,
            "tt0111161,10,2020-01-01,The Shawshank Redemption,https://www.imdb.com/title/tt0111161/,Movie,9.3,142,1994,Drama,2500000,1994-09-23,Frank Darabont"
        ).unwrap();
        writeln!(
            file,
            "tt0944947,9,2020-01-02,Game of Thrones,https://www.imdb.com/title/tt0944947/,TV Series,9.2,57,2011,Action Drama Fantasy,2000000,2011-04-17,"
        ).unwrap();
        file
    }

    fn create_checkins_csv() -> NamedTempFile {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            "Position,Const,Created,Modified,Description,Title,URL,Title Type,IMDb Rating,Runtime (mins),Year,Genres,Num Votes,Release Date,Directors"
        ).unwrap();
        writeln!(
            file,
            "1,tt0111161,2020-01-15,2020-01-15,,The Shawshank Redemption,https://www.imdb.com/title/tt0111161/,Movie,9.3,142,1994,Drama,2500000,1994-09-23,Frank Darabont"
        ).unwrap();
        file
    }

    #[test]
    fn test_parse_watchlist_csv() {
        let file = create_watchlist_csv();
        let items = parse_watchlist_csv(file.path()).unwrap();

        assert_eq!(items.len(), 2);
        assert_eq!(items[0].imdb_id, "tt0111161");
        assert_eq!(items[0].title, "The Shawshank Redemption");
        assert_eq!(items[0].year, Some(1994));
        assert_eq!(items[0].media_type, MediaType::Movie);
        assert_eq!(items[1].imdb_id, "tt0944947");
        assert_eq!(items[1].media_type, MediaType::Show);
    }

    #[test]
    fn test_parse_watchlist_csv_missing_column() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "Title,Year").unwrap();
        writeln!(file, "Test,2020").unwrap();

        let result = parse_watchlist_csv(file.path());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Missing required column"));
    }

    #[test]
    fn test_parse_watchlist_csv_empty_imdb_id() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            "Position,Const,Created,Modified,Description,Title,URL,Title Type,IMDb Rating,Runtime (mins),Year,Genres,Num Votes,Release Date,Directors"
        ).unwrap();
        writeln!(
            file,
            "1,,2020-01-01,2020-01-01,,Test Movie,https://www.imdb.com/title/tt1234567/,Movie,9.3,142,1994,Drama,2500000,1994-09-23,"
        ).unwrap();

        let items = parse_watchlist_csv(file.path()).unwrap();
        assert_eq!(items.len(), 0); // Empty IMDB ID should be filtered
    }

    #[test]
    fn test_parse_ratings_csv() {
        let file = create_ratings_csv();
        let ratings = parse_ratings_csv(file.path()).unwrap();

        assert_eq!(ratings.len(), 2);
        assert_eq!(ratings[0].imdb_id, "tt0111161");
        assert_eq!(ratings[0].rating, 10);
        assert_eq!(ratings[0].source, RatingSource::Imdb);
        assert_eq!(ratings[1].rating, 9);
    }

    #[test]
    fn test_parse_checkins_csv() {
        let file = create_checkins_csv();
        let history = parse_checkins_csv(file.path()).unwrap();

        assert_eq!(history.len(), 1);
        assert_eq!(history[0].imdb_id, "tt0111161");
        assert_eq!(history[0].media_type, MediaType::Movie);
    }
}

