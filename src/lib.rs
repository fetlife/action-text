#![doc = include_str!("../README.md")]

use std::io;

mod extraction;

pub use extraction::Analysis;

use dom_query::{Document, NodeData, NodeId, NodeRef, TreeNode};
use html5ever::serialize::{HtmlSerializer, SerializeOpts, Serializer};

const MENTION_CONTENT_TYPE: &str = "application/vnd.actiontext.mention";
const MAX_WORD_LENGTH: usize = 100;

pub struct RichText {
    document: Document,
    estimated_bytes: usize,
}

impl RichText {
    pub fn parse(source: &str) -> Self {
        let document = Document::fragment(
            source.trim_matches(['\0', '\t', '\n', '\u{b}', '\u{c}', '\r', ' ']),
        );
        document.errors.borrow_mut().clear();
        let estimated_bytes = size_of::<Self>()
            + source.len() * 2
            + document.root().descendants_it().count() * size_of::<TreeNode>();
        Self {
            document,
            estimated_bytes,
        }
    }

    /// Approximate heap size, for hosts that report native memory to their garbage collector.
    pub fn estimated_bytes(&self) -> usize {
        self.estimated_bytes
    }

    /// The first `length` characters as HTML, cut at a word or block boundary, without pictures.
    /// The HTML is not sanitized: callers must sanitize it before rendering.
    pub fn preview_html(&self, length: usize) -> io::Result<String> {
        let cut = self.preview_cut(length);
        let mut serializer = HtmlSerializer::new(
            Vec::new(),
            SerializeOpts {
                scripting_enabled: false,
                ..Default::default()
            },
        );
        let mut open_elements = Vec::new();

        for event in self.walk() {
            match event {
                Event::Enter(node) => {
                    let gallery = attachment_gallery(node);
                    node.query(|node| match &node.data {
                        NodeData::Element(element) => {
                            serializer.start_elem(
                                element.name.clone(),
                                element
                                    .attrs
                                    .iter()
                                    .filter(|_| !gallery)
                                    .map(|attr| (&attr.name, attr.value.as_ref())),
                            )?;
                            open_elements.push(element.name.clone());
                            Ok(())
                        }
                        NodeData::Text { contents } => {
                            let text = match cut {
                                Some((id, keep)) if id == node.id => {
                                    let end = contents
                                        .char_indices()
                                        .nth(keep)
                                        .map_or(contents.len(), |(index, _)| index);
                                    &contents[..end]
                                }
                                _ => contents.as_ref(),
                            };
                            serializer.write_text(text)
                        }
                        NodeData::Comment { contents } => serializer.write_comment(contents),
                        NodeData::ProcessingInstruction { target, contents } => {
                            serializer.write_processing_instruction(target, contents)
                        }
                        NodeData::Doctype { name, .. } => serializer.write_doctype(name),
                        NodeData::Document | NodeData::Fragment => Ok(()),
                    })
                    .transpose()?;
                    if cut.is_some_and(|(id, _)| id == node.id) {
                        break;
                    }
                }
                Event::Exit(node) => {
                    if node.query_or(false, |node| matches!(node.data, NodeData::Element(_)))
                        && let Some(name) = open_elements.pop()
                    {
                        serializer.end_elem(name)?;
                    }
                }
            }
        }
        for name in open_elements.into_iter().rev() {
            serializer.end_elem(name)?;
        }
        String::from_utf8(serializer.writer).map_err(io::Error::other)
    }

    fn preview_cut(&self, length: usize) -> Option<(NodeId, usize)> {
        let mut nodes = Vec::new();
        let mut offset = 0;
        let mut word_break = None;
        for event in self.walk() {
            let Event::Enter(node) = event else { continue };
            let overflow = node.query_or(false, |tree_node| {
                let NodeData::Text { contents } = &tree_node.data else {
                    return false;
                };
                if offset > 0 && starts_block(node) {
                    word_break = Some(offset);
                }
                let start = offset;
                for character in contents.chars() {
                    if character == ' ' {
                        word_break = Some(offset);
                    }
                    offset += 1;
                    if offset > length {
                        break;
                    }
                }
                nodes.push((node.id, offset - start));
                offset > length
            });
            if overflow {
                let mut keep = word_break
                    .filter(|&cut| length - cut <= MAX_WORD_LENGTH)
                    .unwrap_or(length);
                for &(id, count) in &nodes {
                    if count >= keep {
                        return Some((id, keep));
                    }
                    keep -= count;
                }
            }
        }
        None
    }

    fn walk(&self) -> Walk<'_> {
        self.walk_with_pictures(false)
    }

    fn walk_with_pictures(&self, include_pictures: bool) -> Walk<'_> {
        Walk {
            include_pictures,
            pending: self
                .document
                .html_root()
                .first_child()
                .map(Event::Enter)
                .into_iter()
                .collect(),
        }
    }
}

enum Event<'a> {
    Enter(NodeRef<'a>),
    Exit(NodeRef<'a>),
}

struct Walk<'a> {
    pending: Vec<Event<'a>>,
    include_pictures: bool,
}

impl<'a> Iterator for Walk<'a> {
    type Item = Event<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(event) = self.pending.pop() {
            if let Event::Enter(node) = event {
                if let Some(sibling) = node.next_sibling() {
                    self.pending.push(Event::Enter(sibling));
                }
                if !self.include_pictures && picture(node) {
                    continue;
                }
                self.pending.push(Event::Exit(node));
                let child = node
                    .query(|tree_node| match &tree_node.data {
                        NodeData::Element(element)
                            if element.name.local.as_str() == "action-text-attachment" =>
                        {
                            None
                        }
                        NodeData::Element(element) if element.template_contents.is_some() => {
                            element.template_contents.and_then(|id| node.tree.get(&id))
                        }
                        _ => node.first_child(),
                    })
                    .flatten();
                if let Some(child) = child {
                    self.pending.push(Event::Enter(child));
                }
            }
            return Some(event);
        }
        None
    }
}

fn picture(node: NodeRef<'_>) -> bool {
    node.query_or(false, |node| match &node.data {
        NodeData::Element(element) => match element.name.local.as_ref() {
            "img" => true,
            "action-text-attachment" => {
                element.attr_ref("content-type".into()) != Some(MENTION_CONTENT_TYPE)
            }
            _ => false,
        },
        _ => false,
    })
}

fn attachment_gallery(node: NodeRef<'_>) -> bool {
    let div = node.query_or(false, |node| {
        matches!(&node.data, NodeData::Element(element) if element.name.local.as_str() == "div")
    });
    if !div {
        return false;
    }
    let mut attachments = 0;
    for child in node.children_it(false) {
        let allowed = child.query_or(false, |node| match &node.data {
            NodeData::Text { contents } => contents.chars().all(|c| c == ' ' || c == '\n'),
            NodeData::Element(element)
                if element.name.local.as_str() == "action-text-attachment"
                    && element.attr_ref("presentation".into()) == Some("gallery") =>
            {
                attachments += 1;
                true
            }
            _ => false,
        });
        if !allowed {
            return false;
        }
    }
    attachments >= 2
}

fn starts_block(node: NodeRef<'_>) -> bool {
    let mut previous = node.prev_sibling();
    while previous.is_some_and(picture) {
        previous = previous.and_then(|node| node.prev_sibling());
    }
    previous
        .or_else(|| node.parent())
        .is_some_and(is_block_boundary)
}

pub fn is_block_boundary(node: NodeRef<'_>) -> bool {
    node.query_or(false, |node| match &node.data {
        NodeData::Element(element) => matches!(
            element.name.local.as_ref(),
            "p" | "div"
                | "li"
                | "td"
                | "th"
                | "tr"
                | "table"
                | "thead"
                | "tbody"
                | "ul"
                | "ol"
                | "h1"
                | "h2"
                | "h3"
                | "h4"
                | "h5"
                | "h6"
                | "blockquote"
                | "pre"
                | "br"
                | "hr"
                | "figure"
                | "figcaption"
        ),
        _ => false,
    })
}

#[cfg(test)]
mod tests {
    use super::RichText;

    fn preview(source: &str, length: usize) -> String {
        RichText::parse(source).preview_html(length).unwrap()
    }

    #[test]
    fn clips_at_words_and_block_boundaries() {
        assert_eq!(
            preview("<p>abc def ghi</p><p>gone</p>", 7),
            "<p>abc def</p>"
        );
        assert_eq!(
            preview("<p>abc</p><p><img src='x'>def ghi</p>", 4),
            "<p>abc</p>"
        );
        assert_eq!(preview("<p><b>abc</b>def ghi</p>", 4), "<p><b>abc</b>d</p>");
        assert_eq!(
            preview(&format!("<p>{}</p>", "a".repeat(200)), 150),
            format!("<p>{}</p>", "a".repeat(150))
        );
    }

    #[test]
    fn counts_unicode_characters_and_decoded_entities() {
        assert_eq!(preview("<p>😺é&amp;文xyz</p>", 4), "<p>😺é&amp;文</p>");
        assert_eq!(preview("<p>e\u{301}xyz</p>", 2), "<p>e\u{301}</p>");
    }

    #[test]
    fn removes_pictures_and_preserves_empty_mentions() {
        let mention = r#"<action-text-attachment content-type="application/vnd.actiontext.mention" sgid="user"></action-text-attachment>"#;
        let source = format!(
            "<img src='x'><action-text-attachment>ignored</action-text-attachment>{mention}<p>abc</p>"
        );
        assert_eq!(preview(&source, 3), format!("{mention}<p>abc</p>"));
        assert_eq!(
            preview(&mention.replace("</action", "hidden</action"), 1),
            mention
        );
    }

    #[test]
    fn preserves_the_whole_fragment_when_text_fits_exactly() {
        assert_eq!(
            preview("<p>abc</p><!--tail--><hr>", 3),
            "<p>abc</p><!--tail--><hr>"
        );
        assert_eq!(preview("<p>abc</p><!--tail--><hr>def", 3), "<p>abc</p>");
        assert_eq!(preview("<p>abc</p>", 0), "<p></p>");
    }

    #[test]
    fn previews_do_not_mutate_the_document() {
        let document = RichText::parse("<p>abc def ghi</p>");
        assert_eq!(document.preview_html(4).unwrap(), "<p>abc</p>");
        assert_eq!(document.preview_html(100).unwrap(), "<p>abc def ghi</p>");
    }

    #[test]
    fn canonicalizes_gallery_wrappers_before_removing_pictures() {
        let attachment =
            r#"<action-text-attachment presentation="gallery"></action-text-attachment>"#;
        assert_eq!(
            preview(
                &format!("<div class='gallery'>{attachment}\n{attachment}</div>"),
                10
            ),
            "<div>\n</div>"
        );
    }

    #[test]
    fn handles_deep_nesting_without_recursive_traversal() {
        let source = format!("{}abc def{}", "<div>".repeat(2000), "</div>".repeat(2000));
        assert_eq!(preview(&source, 3), source.replace("abc def", "abc"));
    }
}
