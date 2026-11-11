//! The paging controls of the result lists, shared by the tools.

/// One control in a pager. A disabled control, such as "Previous" on the
/// first page, has no URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PagerLink {
    pub label: String,
    pub url: Option<String>,
    /// `prev` or `next`, for markup and assistive technology.
    pub rel: Option<&'static str>,
}

impl PagerLink {
    pub fn new(label: impl Into<String>, url: impl Into<String>) -> Self {
        Self { label: label.into(), url: Some(url.into()), rel: None }
    }

    pub fn previous(label: impl Into<String>, url: impl Into<String>) -> Self {
        Self { rel: Some("prev"), ..Self::new(label, url) }
    }

    pub fn next(label: impl Into<String>, url: impl Into<String>) -> Self {
        Self { rel: Some("next"), ..Self::new(label, url) }
    }

    pub fn disabled(label: impl Into<String>) -> Self {
        Self { label: label.into(), url: None, rel: None }
    }

    pub fn is_disabled(&self) -> bool {
        self.url.is_none()
    }
}

/// The links of a pager, in display order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pager {
    pub links: Vec<PagerLink>,
}

impl Pager {
    pub fn new(links: impl IntoIterator<Item = PagerLink>) -> Self {
        Self { links: links.into_iter().collect() }
    }

    /// Previous and next pages, in that order, omitting the absent one.
    pub fn previous_next(
        previous: Option<String>,
        next: Option<String>,
        previous_label: impl Into<String>,
        next_label: impl Into<String>,
    ) -> Self {
        let mut links = Vec::with_capacity(2);
        if let Some(url) = previous {
            links.push(PagerLink::previous(previous_label, url));
        }
        if let Some(url) = next {
            links.push(PagerLink::next(next_label, url));
        }
        Self { links }
    }

    /// Only a next page, for lists that can go on but not back.
    pub fn next(next: Option<String>, label: impl Into<String>) -> Self {
        let mut links = Vec::with_capacity(1);
        if let Some(url) = next {
            links.push(PagerLink::next(label, url));
        }
        Self { links }
    }

    pub fn is_empty(&self) -> bool {
        self.links.is_empty()
    }

    /// The next page's URL, for messages that link to it inline.
    pub fn next_url(&self) -> Option<&str> {
        self.links.iter().find(|link| link.rel == Some("next")).and_then(|link| link.url.as_deref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn previous_and_next_keep_their_order_and_direction() {
        let pager = Pager::previous_next(Some("/?page=1".into()), Some("/?page=3".into()), "Previous", "Next");
        assert_eq!(pager.links.iter().map(|link| link.label.as_str()).collect::<Vec<_>>(), ["Previous", "Next"]);
        assert_eq!(pager.links[0].rel, Some("prev"));
        assert_eq!(pager.links[1].rel, Some("next"));
        assert_eq!(pager.next_url(), Some("/?page=3"));
    }

    #[test]
    fn absent_pages_are_omitted() {
        assert!(Pager::previous_next(None, None, "Previous", "Next").is_empty());
        let pager = Pager::next(Some("/?page=2".into()), "Older");
        assert_eq!(pager.links.len(), 1);
        assert_eq!(pager.links[0].url.as_deref(), Some("/?page=2"));
        assert_eq!(pager.links[0].rel, Some("next"));
    }

    #[test]
    fn disabled_links_have_no_url() {
        let link = PagerLink::disabled("Start");
        assert!(link.is_disabled());
        assert_eq!(link.url, None);
    }
}
