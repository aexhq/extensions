//! Shared cursor pagination: fetch `limit + 1`, keep `limit`, and cursor on the last kept item.

/// Upper bound shared by every paginated listing in the workspace.
pub const MAX_PAGE: usize = 100;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
}

pub trait PageIdentity {
    fn page_identity(&self) -> &str;
}

/// Trims an over-fetched (`limit + 1`) result set to one page.
pub fn page<T>(mut items: Vec<T>, limit: usize) -> Page<T>
where
    T: PageIdentity,
{
    let next_cursor = (items.len() > limit).then(|| items[limit - 1].page_identity().to_owned());
    items.truncate(limit);
    Page { items, next_cursor }
}

#[cfg(test)]
mod tests {
    use super::*;

    impl PageIdentity for String {
        fn page_identity(&self) -> &str {
            self
        }
    }

    #[test]
    fn an_overfetched_set_cursors_on_the_last_kept_item() {
        let result = page(vec!["a".to_string(), "b".into(), "c".into()], 2);
        assert_eq!(result.items, vec!["a".to_string(), "b".into()]);
        assert_eq!(result.next_cursor.as_deref(), Some("b"));
    }

    #[test]
    fn a_final_page_has_no_cursor() {
        let result = page(vec!["a".to_string()], 2);
        assert_eq!(result.items.len(), 1);
        assert_eq!(result.next_cursor, None);
    }
}
