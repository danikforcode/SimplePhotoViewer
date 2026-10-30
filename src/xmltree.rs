// SimplePhotoViewer (Rust/Win32) — Copyright (C) 2026 Даниил Кочубейник
// Лицензия: GNU GPL v3 или новее. Гарантий нет.
//! Минимальное XML-дерево поверх quick-xml: для разбора DOCX/ODT достаточно.
//! Имена хранятся без префикса пространства имён («w:p» → «p»).

use quick_xml::events::Event;

#[derive(Debug, Default, Clone)]
pub struct Node {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Node>,
    /// Для текстового узла (name == "#text") — сам текст.
    pub text: String,
}

impl Node {
    pub fn attr(&self, local: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k == local).map(|(_, v)| v.as_str())
    }
    pub fn child(&self, name: &str) -> Option<&Node> {
        self.children.iter().find(|c| c.name == name)
    }
    pub fn kids<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Node> + 'a {
        self.children.iter().filter(move |c| c.name == name)
    }
    /// Первый потомок с таким именем на любой глубине.
    pub fn find(&self, name: &str) -> Option<&Node> {
        for c in &self.children {
            if c.name == name {
                return Some(c);
            }
            if let Some(f) = c.find(name) {
                return Some(f);
            }
        }
        None
    }
    /// Весь текст поддерева.
    pub fn all_text(&self) -> String {
        let mut s = String::new();
        self.collect_text(&mut s);
        s
    }
    fn collect_text(&self, s: &mut String) {
        if self.name == "#text" {
            s.push_str(&self.text);
        }
        for c in &self.children {
            c.collect_text(s);
        }
    }
    pub fn is_text(&self) -> bool {
        self.name == "#text"
    }
}

fn local(s: &str) -> String {
    match s.rfind(':') {
        Some(i) => s[i + 1..].to_string(),
        None => s.to_string(),
    }
}

fn entity(name: &str) -> Option<String> {
    if let Some(num) = name.strip_prefix('#') {
        let v = if let Some(h) = num.strip_prefix('x').or_else(|| num.strip_prefix('X')) {
            u32::from_str_radix(h, 16).ok()?
        } else {
            num.parse().ok()?
        };
        return char::from_u32(v).map(|c| c.to_string());
    }
    Some(
        match name {
            "amp" => "&",
            "lt" => "<",
            "gt" => ">",
            "quot" => "\"",
            "apos" => "'",
            "nbsp" => "\u{a0}",
            _ => return None,
        }
        .to_string(),
    )
}

/// Разобрать XML в дерево. Корень — искусственный узел с именем "".
pub fn parse(xml: &str) -> Result<Node, String> {
    let mut r = quick_xml::Reader::from_str(xml);
    let mut stack: Vec<Node> = vec![Node::default()];
    let open = |e: &quick_xml::events::BytesStart| -> Node {
        let attrs = e
            .attributes()
            .flatten()
            .map(|a| (local(a.key.local_name().as_ref()), a.normalized_value(quick_xml::XmlVersion::Implicit1_0).map(|v| v.to_string()).unwrap_or_else(|_| a.value.to_string())))
            .collect();
        Node { name: local(e.name().as_ref()), attrs, ..Default::default() }
    };
    let push_text = |stack: &mut Vec<Node>, t: &str| {
        let top = stack.last_mut().unwrap();
        if let Some(last) = top.children.last_mut().filter(|c| c.is_text()) {
            last.text.push_str(t);
        } else {
            top.children.push(Node { name: "#text".into(), text: t.to_string(), ..Default::default() });
        }
    };
    loop {
        match r.read_event() {
            Ok(Event::Start(e)) => stack.push(open(&e)),
            Ok(Event::Empty(e)) => {
                let n = open(&e);
                stack.last_mut().unwrap().children.push(n);
            }
            Ok(Event::End(_)) => {
                if stack.len() > 1 {
                    let n = stack.pop().unwrap();
                    stack.last_mut().unwrap().children.push(n);
                }
            }
            Ok(Event::Text(t)) => push_text(&mut stack, &t),
            Ok(Event::CData(t)) => push_text(&mut stack, t.as_ref()),
            Ok(Event::GeneralRef(rf)) => {
                if let Some(s) = entity(&rf) {
                    push_text(&mut stack, &s);
                }
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(e) => return Err(format!("XML повреждён: {e}")),
        }
    }
    while stack.len() > 1 {
        let n = stack.pop().unwrap();
        stack.last_mut().unwrap().children.push(n);
    }
    Ok(stack.pop().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tree() {
        let x = r#"<?xml version="1.0"?><w:doc xmlns:w="x"><w:p w:val="1">a &amp; b<w:t>c&#1103;</w:t><w:br/></w:p></w:doc>"#;
        let root = parse(x).unwrap();
        let doc = root.child("doc").unwrap();
        let p = doc.child("p").unwrap();
        assert_eq!(p.attr("val"), Some("1"));
        assert_eq!(p.children[0].text, "a & b");
        assert_eq!(p.all_text(), "a & bcя");
        assert!(p.child("br").is_some());
        assert!(root.find("t").is_some());
    }
}
