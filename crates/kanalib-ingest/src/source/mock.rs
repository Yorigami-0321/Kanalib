//! 测试与 box 冒烟用的内存来源；不发任何外部请求。

use super::{
    AiType, BoxFuture, ImageRef, Page, Rating, SourceAdapter, SourceAuthor, SourceCapabilities,
    SourceError, SourceTag, SourceWork,
};

#[derive(Clone, Debug)]
pub struct MockPage {
    pub work: SourceWork,
    pub image_url: String,
}

#[derive(Clone, Debug, Default)]
pub struct MockSource {
    pub id: &'static str,
    pub bookmark_pages: Vec<Vec<MockPage>>,
    pub author_works: Vec<SourceWork>,
    pub works: Vec<SourceWork>,
    pub deleted_work_ids: Vec<String>,
    pub authors: Vec<SourceAuthor>,
    pub image_bytes: Vec<u8>,
}

impl MockSource {
    pub fn new(id: &'static str) -> Self {
        Self {
            id,
            ..Self::default()
        }
    }

    pub fn work(external_id: &str, page_count: u32) -> SourceWork {
        SourceWork {
            external_id: external_id.to_string(),
            author: Some(SourceAuthor {
                external_id: "mock-author".to_string(),
                name: "Mock Author".to_string(),
                avatar_url: None,
            }),
            title: format!("Work {external_id}"),
            page_count,
            posted_at: Some(1_700_000_000),
            rating: Rating::All,
            x_restrict: Some(0),
            ai: AiType::Unknown,
            sanity_level: None,
            restriction_attributes: Vec::new(),
            tags: vec![SourceTag {
                name: "mock".to_string(),
                category: "general".to_string(),
                score: 0.75,
                translated_name: None,
            }],
            is_ugoira: false,
        }
    }

    pub fn page(external_id: &str, page: u32) -> MockPage {
        MockPage {
            work: Self::work(external_id, 1),
            image_url: format!("mock://image/{external_id}/{page}"),
        }
    }
}

impl SourceAdapter for MockSource {
    fn id(&self) -> &'static str {
        self.id
    }

    fn capabilities(&self) -> SourceCapabilities {
        SourceCapabilities {
            bookmarks: true,
            author_works: true,
            multiple_pages: true,
            ugoira: false,
        }
    }

    fn fetch_work(&self, external_id: &str) -> BoxFuture<'_, Result<SourceWork, SourceError>> {
        let result = self
            .works
            .iter()
            .find(|work| work.external_id == external_id)
            .cloned()
            .or_else(|| {
                self.bookmark_pages
                    .iter()
                    .flatten()
                    .map(|page| &page.work)
                    .find(|work| work.external_id == external_id)
                    .cloned()
            })
            .ok_or_else(|| SourceError::not_found(format!("unknown work {external_id}")));
        Box::pin(async move { result })
    }

    fn fetch_author(&self, external_id: &str) -> BoxFuture<'_, Result<SourceAuthor, SourceError>> {
        let result = self
            .authors
            .iter()
            .find(|author| author.external_id == external_id)
            .cloned()
            .ok_or_else(|| SourceError::not_found(format!("unknown author {external_id}")));
        Box::pin(async move { result })
    }

    fn list_author_works(
        &self,
        _external_id: &str,
        cursor: Option<&str>,
    ) -> BoxFuture<'_, Result<Page<SourceWork>, SourceError>> {
        let offset = cursor
            .and_then(|cursor| cursor.parse::<usize>().ok())
            .unwrap_or(0);
        let items = self
            .author_works
            .iter()
            .skip(offset)
            .take(2)
            .cloned()
            .collect();
        let next = if offset + 2 < self.author_works.len() {
            Some((offset + 2).to_string())
        } else {
            None
        };
        Box::pin(async move {
            Ok(Page {
                items,
                next_cursor: next,
            })
        })
    }

    fn list_public_bookmarks(
        &self,
        _user_id: &str,
        cursor: Option<&str>,
    ) -> BoxFuture<'_, Result<Page<SourceWork>, SourceError>> {
        let index = cursor
            .and_then(|cursor| cursor.parse::<usize>().ok())
            .unwrap_or(0);
        let result = self.bookmark_pages.get(index).map(|works| Page {
            items: works.iter().map(|page| page.work.clone()).collect(),
            next_cursor: if index + 1 < self.bookmark_pages.len() {
                Some((index + 1).to_string())
            } else {
                None
            },
        });
        Box::pin(
            async move { result.ok_or_else(|| SourceError::internal("invalid bookmark cursor")) },
        )
    }

    fn resolve_image(
        &self,
        work: &SourceWork,
        page: u32,
    ) -> BoxFuture<'_, Result<ImageRef, SourceError>> {
        let page_count = work.page_count;
        let external_id = work.external_id.clone();
        let result = self
            .bookmark_pages
            .iter()
            .flatten()
            .find(|candidate| candidate.work.external_id == external_id)
            .map(|candidate| ImageRef::new(candidate.image_url.clone()).ext("png"));
        Box::pin(async move {
            result.ok_or_else(|| {
                SourceError::not_found(format!(
                    "no image for {}:{page} of {page_count}",
                    external_id
                ))
            })
        })
    }
}
