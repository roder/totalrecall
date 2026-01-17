#[cfg(test)]
mod tests {
    use super::*;
    use media_sync_models::{MediaType, Rating, RatingSource, WatchHistory, WatchlistItem};
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






