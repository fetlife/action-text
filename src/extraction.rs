use std::collections::{BTreeMap, HashSet};

use dom_query::{Element, NodeData};
use serde::Serialize;

use super::{Event, RichText};

/// Text counts exclude attachments; SGIDs and media attributes are unverified source metadata.
#[derive(Default, Serialize)]
pub struct Analysis {
    text_length: usize,
    text_bytes: usize,
    has_text: bool,
    has_images: bool,
    links: Vec<String>,
    alt_texts: Vec<String>,
    attachments: Vec<BTreeMap<String, String>>,
    images: Vec<Image>,
}

#[derive(Serialize)]
struct Image {
    src: Option<String>,
    alt: Option<String>,
}

struct Frame {
    content_start: usize,
    element_children: usize,
}

#[derive(Default)]
struct Extraction {
    text: String,
    frames: Vec<Frame>,
    lists: Vec<bool>,
    hidden: usize,
    attachment_count: usize,
    analysis: Option<Analysis>,
    links: HashSet<String>,
}

impl RichText {
    /// Plain text like Action Text's `to_plain_text`, with h2–h6 as blocks like h1
    /// and attachments (mentions, pictures) omitted.
    pub fn plain_text(&self) -> String {
        self.extract(false, None).text
    }

    /// Plain text with each attachment replaced by the literal text given for it,
    /// in document order.
    pub fn plain_text_with_attachments(
        &self,
        attachment_texts: &[String],
    ) -> Result<String, &'static str> {
        let result = self.extract(false, Some(attachment_texts));
        if attachment_texts.len() != result.attachment_count {
            return Err(
                "attachment_texts must contain one string per attachment, in document order",
            );
        }
        Ok(result.text)
    }

    pub fn analysis(&self) -> Analysis {
        let result = self.extract(true, None);
        let mut analysis = result.analysis.unwrap_or_default();
        analysis.text_length = result.text.chars().count();
        analysis.text_bytes = result.text.len();
        analysis
    }

    fn extract(&self, analyze: bool, attachment_texts: Option<&[String]>) -> Extraction {
        let mut output = Extraction {
            analysis: analyze.then(Analysis::default),
            ..Default::default()
        };
        for event in self.walk_with_pictures(true) {
            match event {
                Event::Enter(node) => {
                    node.query(|node| match &node.data {
                        NodeData::Element(element) => output.enter(element, attachment_texts),
                        NodeData::Text { contents } if output.hidden == 0 => {
                            output.text.push_str(chomp(contents));
                            if let Some(analysis) = &mut output.analysis {
                                analysis.has_text |= !blank(contents);
                            }
                        }
                        NodeData::Document | NodeData::Fragment => output.push_frame(),
                        _ => (),
                    });
                }
                Event::Exit(node) => {
                    node.query(|node| match &node.data {
                        NodeData::Element(element) => output.leave(element.name.local.as_ref()),
                        NodeData::Document | NodeData::Fragment => {
                            output.frames.pop();
                        }
                        _ => (),
                    });
                }
            }
        }
        output.trim_newlines(0);
        output
    }
}

impl Extraction {
    fn push_frame(&mut self) {
        self.frames.push(Frame {
            content_start: self.text.len(),
            element_children: 0,
        });
    }

    fn enter(&mut self, element: &Element, attachment_texts: Option<&[String]>) {
        let name = element.name.local.as_ref();
        let ordinal = if let Some(parent) = self.frames.last_mut() {
            // ActionText numbers list items after replacing attachments with text.
            if name != "action-text-attachment" || attachment_texts.is_none() {
                parent.element_children += 1;
            }
            parent.element_children
        } else {
            1
        };
        self.analyze(element);
        if matches!(name, "script" | "style") {
            self.hidden += 1;
        }
        if self.hidden == 0 {
            match name {
                "li" => {
                    for _ in 1..self.lists.len() {
                        self.text.push_str("  ");
                    }
                    if self.lists.last() == Some(&true) {
                        self.text.push_str(&format!("{ordinal}. "));
                    } else {
                        self.text.push_str("• ");
                    }
                }
                "ol" | "ul" if !self.lists.is_empty() => self.text.push('\n'),
                "figcaption" => self.text.push('['),
                "br" => self.text.push('\n'),
                "action-text-attachment" => {
                    if let Some(text) =
                        attachment_texts.and_then(|texts| texts.get(self.attachment_count))
                    {
                        self.text.push_str(chomp(text));
                    }
                }
                _ => (),
            }
        }
        if name == "action-text-attachment" {
            self.attachment_count += 1;
        }
        if matches!(name, "ol" | "ul") {
            self.lists.push(name == "ol");
        }
        self.push_frame();
    }

    fn leave(&mut self, name: &str) {
        let Some(frame) = self.frames.pop() else {
            return;
        };
        if self.hidden == 0 {
            match name {
                "p" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "ol" | "ul" | "blockquote" => {
                    self.trim_newlines(frame.content_start);
                    self.text.push_str("\n\n");
                    if name == "blockquote" {
                        self.quote(frame.content_start);
                    }
                }
                "div" | "li" => {
                    self.trim_newlines(frame.content_start);
                    self.text.push('\n');
                }
                "figcaption" => {
                    self.trim_newlines(frame.content_start);
                    self.text.push(']');
                }
                _ => (),
            }
        }
        if matches!(name, "ol" | "ul") {
            self.lists.pop();
        }
        if matches!(name, "script" | "style") {
            self.hidden -= 1;
        }
    }

    fn trim_newlines(&mut self, start: usize) {
        let length = chomp(&self.text[start..]).len();
        self.text.truncate(start + length);
    }

    fn quote(&mut self, start: usize) {
        let text = &self.text[start..];
        if blank(text) {
            self.text.truncate(start);
            self.text.push_str("“”");
            return;
        }
        let first = text
            .char_indices()
            .find(|(_, c)| !whitespace(*c))
            .map(|(i, _)| i);
        let last = text
            .char_indices()
            .rev()
            .find(|(_, c)| !whitespace(*c))
            .map(|(i, c)| i + c.len_utf8());
        if let (Some(first), Some(last)) = (first, last) {
            self.text.insert(start + last, '”');
            self.text.insert(start + first, '“');
        }
    }

    fn analyze(&mut self, element: &Element) {
        let Some(analysis) = &mut self.analysis else {
            return;
        };
        match element.name.local.as_ref() {
            "a" => {
                if let Some(href) = attribute(element, "href")
                    && self.links.insert(href.to_owned())
                {
                    analysis.links.push(href.to_owned());
                }
            }
            "action-text-attachment" => {
                if let Some(alt) = attribute(element, "alt").filter(|alt| !blank(alt)) {
                    analysis.alt_texts.push(alt.to_owned());
                }
                analysis.has_images |= attribute(element, "content-type")
                    .is_some_and(|kind| kind.starts_with("image/"));
                analysis.attachments.push(
                    element
                        .attrs
                        .iter()
                        .map(|attr| (attr.name.local.to_string(), attr.value.to_string()))
                        .collect(),
                );
            }
            "img" => {
                analysis.has_images = true;
                analysis.images.push(Image {
                    src: attribute(element, "src").map(str::to_owned),
                    alt: attribute(element, "alt").map(str::to_owned),
                });
            }
            _ => (),
        }
    }
}

fn attribute<'a>(element: &'a Element, name: &str) -> Option<&'a str> {
    element
        .attrs
        .iter()
        .find(|attr| attr.name.ns.as_str().is_empty() && attr.name.local.as_str() == name)
        .map(|attr| attr.value.as_ref())
}

// Ruby's `\s`, which Action Text uses to place quotes.
fn whitespace(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\u{b}' | '\u{c}')
}

// Rails' `blank?`, which also counts Unicode spaces such as `&nbsp;`.
fn blank(text: &str) -> bool {
    text.chars().all(char::is_whitespace)
}

fn chomp(mut text: &str) -> &str {
    while let Some(stripped) = text.strip_suffix('\n') {
        text = stripped.strip_suffix('\r').unwrap_or(stripped);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::RichText;

    fn plain(source: &str) -> String {
        RichText::parse(source).plain_text()
    }

    #[test]
    fn preserves_action_text_block_and_list_formatting() {
        assert_eq!(
            plain("<h2>Title</h2><p>Body<br>line</p><div>last</div>"),
            "Title\n\nBody\nline\n\nlast"
        );
        assert_eq!(
            plain("<ol start='4'><li>a<ul><li>b</li></ul></li><li>c</li></ol>"),
            "1. a\n  • b\n2. c"
        );
        assert_eq!(plain("<p>a</p><div></div>b"), "a\n\n\nb");
        assert_eq!(plain("<blockquote> </blockquote>x"), "“”x");
        assert_eq!(plain("<blockquote>&nbsp;</blockquote>x"), "“”x");
        assert_eq!(
            plain("<blockquote>&nbsp;a&nbsp;</blockquote>"),
            "“\u{a0}a\u{a0}”"
        );
        assert_eq!(
            plain("<figure><figcaption>Caption<br></figcaption></figure>"),
            "[Caption]"
        );
    }

    #[test]
    fn omits_hidden_content_and_preserves_literal_text() {
        assert_eq!(
            plain(
                "<p>&lt;script&gt;x&lt;/script&gt; &amp; 😺</p><script>gone</script><style>gone</style>"
            ),
            "<script>x</script> & 😺"
        );
        assert_eq!(plain("<pre>a\nb\n\n</pre>x"), "a\nbx");
    }

    #[test]
    fn accepts_literal_attachment_text_without_changing_the_document() {
        let document = RichText::parse(
            "<p>Hi <action-text-attachment sgid='user'></action-text-attachment>!</p>",
        );
        assert_eq!(
            document
                .plain_text_with_attachments(&["@name<&>".to_owned()])
                .unwrap(),
            "Hi @name<&>!"
        );
        assert_eq!(document.plain_text(), "Hi !");
        assert!(document.plain_text_with_attachments(&[]).is_err());
    }

    #[test]
    fn numbers_list_items_after_replacing_attachments_with_text() {
        let attachment = "<action-text-attachment></action-text-attachment>";
        let document = RichText::parse(&format!(
            "<ol>{attachment}<li>First</li>{attachment}<li>Second</li></ol>"
        ));

        assert_eq!(
            document
                .plain_text_with_attachments(&["Intro".to_owned(), String::new()])
                .unwrap(),
            "Intro1. First\n2. Second"
        );
        assert_eq!(document.plain_text(), "2. First\n4. Second");
    }

    #[test]
    fn reports_raw_references_links_and_attachment_alt_text() {
        let document = RichText::parse(
            "<p>😺 <a href='https://example.org?a=1&amp;b=2'>link</a><a href='https://example.org?a=1&amp;b=2'>again</a></p><action-text-attachment sgid='unverified' content-type='image/jpeg' alt='description'><a href='ignored'>hidden</a></action-text-attachment><img src='#image:1' alt='legacy'>",
        );
        let analysis = document.analysis();
        assert_eq!(analysis.links, ["https://example.org?a=1&b=2"]);
        assert_eq!(analysis.alt_texts, ["description"]);
        assert_eq!(analysis.attachments[0]["sgid"], "unverified");
        assert_eq!(analysis.images[0].src.as_deref(), Some("#image:1"));
        assert!(analysis.has_text && analysis.has_images);
        assert_eq!(analysis.text_length, "😺 linkagain".chars().count());
        assert_eq!(analysis.text_bytes, "😺 linkagain".len());
    }

    #[test]
    fn distinguishes_formatting_from_text_and_handles_deep_nesting() {
        assert!(
            !RichText::parse("<ul><li></li></ul><p>&nbsp;</p><script>hidden</script>")
                .analysis()
                .has_text
        );
        assert_eq!(
            plain(&format!(
                "{}text{}",
                "<div>".repeat(2000),
                "</div>".repeat(2000)
            )),
            "text"
        );
    }

    #[test]
    fn retains_namespaced_attachment_attributes_like_nokogiri() {
        let document = RichText::parse(
            "<svg><action-text-attachment caption='caption' xlink:href='reference'>child</action-text-attachment></svg>",
        );
        assert_eq!(document.analysis().attachments[0]["href"], "reference");
        assert_eq!(document.plain_text(), "");
    }
}
