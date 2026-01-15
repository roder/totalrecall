#[cfg(test)]
mod tests {
    use super::*;
    use media_sync_models::{MediaType, Rating, RatingSource, WatchHistory, WatchlistItem};
    use chrono::Utc;

    fn create_watchlist_item(imdb_id: &str, title: &str) -> WatchlistItem {
        WatchlistItem {
            imdb_id: imdb_id.to_string(),
            ids: None,
            title: title.to_string(),
            year: Some(2020),
            media_type: MediaType::Movie,
            date_added: Utc::now(),
            source: "test".to_string(),
            status: None,
        }
    }

    fn create_rating(imdb_id: &str, rating: u8) -> Rating {
        Rating {
            imdb_id: imdb_id.to_string(),
            ids: None,
            rating,
            date_added: Utc::now(),
            media_type: MediaType::Movie,
            source: RatingSource::Imdb,
        }
    }

    #[test]
    fn test_filter_items_by_imdb_id() {
        let source = vec![
            create_watchlist_item("tt001", "Movie 1"),
            create_watchlist_item("tt002", "Movie 2"),
            create_watchlist_item("tt003", "Movie 3"),
        ];
        let target = vec![
            create_watchlist_item("tt001", "Movie 1"),
            create_watchlist_item("tt002", "Movie 2"),
        ];

        let filtered = filter_items_by_imdb_id(&source, &target);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].imdb_id, "tt003");
    }

    #[test]
    fn test_filter_items_by_imdb_id_empty_target() {
        let source = vec![
            create_watchlist_item("tt001", "Movie 1"),
            create_watchlist_item("tt002", "Movie 2"),
        ];
        let target = vec![];

        let filtered = filter_items_by_imdb_id(&source, &target);
        assert_eq!(filtered.len(), 2);
    }

    #[test]
    fn test_filter_items_by_imdb_id_empty_source() {
        let source = vec![];
        let target = vec![create_watchlist_item("tt001", "Movie 1")];

        let filtered = filter_items_by_imdb_id(&source, &target);
        assert_eq!(filtered.len(), 0);
    }

    #[test]
    fn test_filter_items_by_imdb_id_with_empty_imdb_id() {
        let source = vec![
            create_watchlist_item("tt001", "Movie 1"),
            create_watchlist_item("", "Movie 2"), // Empty IMDB ID
        ];
        let target = vec![];

        let filtered = filter_items_by_imdb_id(&source, &target);
        assert_eq!(filtered.len(), 1); // Empty IMDB ID should be filtered
        assert_eq!(filtered[0].imdb_id, "tt001");
    }

    #[test]
    fn test_remove_duplicates_by_imdb_id() {
        let items = vec![
            create_watchlist_item("tt001", "Movie 1"),
            create_watchlist_item("tt001", "Movie 1 Duplicate"),
            create_watchlist_item("tt002", "Movie 2"),
            create_watchlist_item("tt002", "Movie 2 Duplicate"),
        ];

        let deduped = remove_duplicates_by_imdb_id(items);
        assert_eq!(deduped.len(), 2);
        assert_eq!(deduped[0].imdb_id, "tt001");
        assert_eq!(deduped[1].imdb_id, "tt002");
    }

    #[test]
    fn test_remove_duplicates_by_imdb_id_with_empty_ids() {
        let items = vec![
            create_watchlist_item("tt001", "Movie 1"),
            create_watchlist_item("", "Movie 2"),
            create_watchlist_item("", "Movie 3"),
        ];

        let deduped = remove_duplicates_by_imdb_id(items);
        assert_eq!(deduped.len(), 1); // Empty IDs are filtered
        assert_eq!(deduped[0].imdb_id, "tt001");
    }

    #[test]
    fn test_filter_missing_imdb_ids() {
        let items = vec![
            create_watchlist_item("tt001", "Movie 1"),
            create_watchlist_item("", "Movie 2"),
            create_watchlist_item("tt003", "Movie 3"),
        ];

        let filtered = filter_missing_imdb_ids(items);
        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].imdb_id, "tt001");
        assert_eq!(filtered[1].imdb_id, "tt003");
    }

    #[test]
    fn test_filter_items_by_imdb_id_with_ratings() {
        let source = vec![
            create_rating("tt001", 10),
            create_rating("tt002", 9),
            create_rating("tt003", 8),
        ];
        let target = vec![
            create_rating("tt001", 10),
        ];

        let filtered = filter_items_by_imdb_id(&source, &target);
        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].imdb_id, "tt002");
        assert_eq!(filtered[1].imdb_id, "tt003");
    }
}





