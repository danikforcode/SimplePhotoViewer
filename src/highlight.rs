// SimplePhotoViewer (Rust/Win32) — Copyright (C) 2026 Даниил Кочубейник
// Лицензия: GNU GPL v3 или новее. Гарантий нет.
//! Лёгкая подсветка синтаксиса: комментарии, строки, числа, ключевые слова,
//! теги/ключи. Без грамматик — по семействам языков. Работает с UTF-16
//! (как текст в RichEdit), позиции — индексы UTF-16. Чистая логика + тесты.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tok {
    Comment,
    Str,
    Num,
    Kw,
    Type,
    Tag,
    Attr,
    Key,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Span {
    pub start: usize,
    pub len: usize,
    pub tok: Tok,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lang {
    Plain,
    C,      // C, C++
    CSharp,
    Java,
    Js,     // JS/TS/JSON5/Vue
    Rust,
    Go,
    Kotlin,
    Swift,
    Dart,
    Php,
    Python,
    Shell,
    PowerShell,
    Bat,
    Sql,
    Lua,
    Ruby,
    Xml,    // XML/HTML/SVG/XAML
    Css,
    Json,
    Yaml,
    Toml,
    Ini,
    Markdown,
}

pub fn lang_for(ext: &str, name: &str) -> Lang {
    match ext {
        "c" | "h" | "cpp" | "hpp" | "cc" | "cxx" | "hxx" | "ino" => Lang::C,
        "cs" => Lang::CSharp,
        "java" | "gradle" => Lang::Java,
        "js" | "mjs" | "cjs" | "jsx" | "ts" | "tsx" | "vue" | "json5" => Lang::Js,
        "rs" => Lang::Rust,
        "go" => Lang::Go,
        "kt" | "kts" => Lang::Kotlin,
        "swift" => Lang::Swift,
        "dart" => Lang::Dart,
        "php" => Lang::Php,
        "py" | "pyw" | "pyi" => Lang::Python,
        "sh" | "bash" | "zsh" | "env" | "cmake" | "r" | "pl" | "gitignore" | "gitattributes" | "dockerfile" => Lang::Shell,
        "ps1" | "psm1" | "psd1" => Lang::PowerShell,
        "bat" | "cmd" => Lang::Bat,
        "sql" => Lang::Sql,
        "lua" => Lang::Lua,
        "rb" => Lang::Ruby,
        "xml" | "xaml" | "csproj" | "vcxproj" | "props" | "targets" | "plist" | "svg" | "html" | "htm" | "xhtml" => Lang::Xml,
        "css" | "scss" | "less" => Lang::Css,
        "json" | "jsonc" | "lock" => Lang::Json,
        "yaml" | "yml" => Lang::Yaml,
        "toml" => Lang::Toml,
        "ini" | "cfg" | "conf" | "config" | "properties" | "reg" | "editorconfig" | "inf" => Lang::Ini,
        "md" | "markdown" => Lang::Markdown,
        _ => match name {
            "makefile" | "dockerfile" | "cmakelists.txt" => Lang::Shell,
            _ => Lang::Plain,
        },
    }
}

fn keywords(l: Lang) -> &'static [&'static str] {
    match l {
        Lang::C => &["auto", "break", "case", "char", "const", "continue", "default", "do", "double", "else", "enum", "extern", "float", "for", "goto", "if", "inline", "int", "long", "register", "return", "short", "signed", "sizeof", "static", "struct", "switch", "typedef", "union", "unsigned", "void", "volatile", "while", "bool", "true", "false", "nullptr", "NULL", "class", "public", "private", "protected", "virtual", "override", "template", "typename", "namespace", "using", "new", "delete", "this", "throw", "try", "catch", "constexpr", "auto", "noexcept", "operator", "friend", "explicit", "mutable", "static_cast", "dynamic_cast", "reinterpret_cast", "const_cast"],
        Lang::CSharp => &["abstract", "as", "async", "await", "base", "bool", "break", "byte", "case", "catch", "char", "class", "const", "continue", "decimal", "default", "delegate", "do", "double", "else", "enum", "event", "false", "finally", "float", "for", "foreach", "get", "if", "in", "int", "interface", "internal", "is", "long", "namespace", "new", "null", "object", "out", "override", "params", "private", "protected", "public", "readonly", "record", "ref", "return", "sealed", "set", "static", "string", "struct", "switch", "this", "throw", "true", "try", "typeof", "using", "var", "virtual", "void", "while", "yield"],
        Lang::Java => &["abstract", "boolean", "break", "byte", "case", "catch", "char", "class", "continue", "default", "do", "double", "else", "enum", "extends", "final", "finally", "float", "for", "if", "implements", "import", "instanceof", "int", "interface", "long", "new", "null", "package", "private", "protected", "public", "return", "short", "static", "super", "switch", "synchronized", "this", "throw", "throws", "true", "false", "try", "var", "void", "while", "record"],
        Lang::Js => &["async", "await", "break", "case", "catch", "class", "const", "continue", "debugger", "default", "delete", "do", "else", "export", "extends", "false", "finally", "for", "from", "function", "if", "import", "in", "instanceof", "let", "new", "null", "of", "return", "static", "super", "switch", "this", "throw", "true", "try", "typeof", "undefined", "var", "void", "while", "yield", "interface", "type", "enum", "implements", "private", "public", "protected", "readonly", "as", "any", "number", "string", "boolean"],
        Lang::Rust => &["as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true", "type", "unsafe", "use", "where", "while", "i8", "i16", "i32", "i64", "i128", "isize", "u8", "u16", "u32", "u64", "u128", "usize", "f32", "f64", "bool", "char", "str"],
        Lang::Go => &["break", "case", "chan", "const", "continue", "default", "defer", "else", "fallthrough", "for", "func", "go", "goto", "if", "import", "interface", "map", "package", "range", "return", "select", "struct", "switch", "type", "var", "nil", "true", "false", "int", "string", "bool", "error", "byte", "rune", "float64"],
        Lang::Kotlin => &["as", "break", "class", "continue", "do", "else", "false", "for", "fun", "if", "in", "interface", "is", "null", "object", "package", "return", "super", "this", "throw", "true", "try", "typealias", "val", "var", "when", "while", "import", "private", "public", "override", "data", "sealed", "suspend"],
        Lang::Swift => &["class", "deinit", "enum", "extension", "func", "import", "init", "let", "protocol", "struct", "subscript", "typealias", "var", "break", "case", "continue", "default", "do", "else", "for", "guard", "if", "in", "repeat", "return", "switch", "where", "while", "as", "false", "is", "nil", "self", "super", "throw", "throws", "true", "try", "public", "private", "static"],
        Lang::Dart => &["abstract", "async", "await", "break", "case", "catch", "class", "const", "continue", "default", "do", "else", "enum", "extends", "false", "final", "finally", "for", "if", "import", "in", "is", "late", "new", "null", "required", "return", "static", "super", "switch", "this", "throw", "true", "try", "var", "void", "while", "with"],
        Lang::Php => &["abstract", "and", "array", "as", "break", "case", "catch", "class", "const", "continue", "default", "do", "echo", "else", "elseif", "extends", "false", "final", "for", "foreach", "function", "if", "implements", "include", "interface", "isset", "namespace", "new", "null", "or", "private", "protected", "public", "require", "return", "static", "switch", "this", "throw", "true", "try", "use", "var", "while"],
        Lang::Python => &["False", "None", "True", "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del", "elif", "else", "except", "finally", "for", "from", "global", "if", "import", "in", "is", "lambda", "nonlocal", "not", "or", "pass", "raise", "return", "try", "while", "with", "yield", "self", "match", "case"],
        Lang::Shell => &["if", "then", "else", "elif", "fi", "for", "while", "do", "done", "case", "esac", "in", "function", "return", "export", "local", "echo", "exit", "set", "unset", "source", "true", "false", "FROM", "RUN", "CMD", "COPY", "ADD", "WORKDIR", "ENV", "EXPOSE", "ENTRYPOINT", "ARG", "LABEL", "VOLUME", "USER"],
        Lang::PowerShell => &["begin", "break", "catch", "class", "continue", "do", "else", "elseif", "end", "enum", "exit", "filter", "finally", "for", "foreach", "function", "if", "in", "param", "process", "return", "switch", "throw", "trap", "try", "until", "using", "while"],
        Lang::Bat => &["echo", "set", "if", "else", "goto", "call", "for", "in", "do", "exit", "not", "exist", "defined", "errorlevel", "setlocal", "endlocal", "pause", "cd", "start", "shift", "off", "on"],
        Lang::Sql => &["select", "from", "where", "and", "or", "not", "insert", "into", "values", "update", "set", "delete", "create", "table", "drop", "alter", "add", "index", "primary", "key", "foreign", "references", "join", "left", "right", "inner", "outer", "on", "as", "group", "by", "order", "having", "limit", "offset", "distinct", "null", "is", "in", "like", "between", "case", "when", "then", "else", "end", "union", "all", "exists", "view", "default", "int", "integer", "varchar", "text", "date", "unique", "asc", "desc", "count", "sum", "avg", "min", "max"],
        Lang::Lua => &["and", "break", "do", "else", "elseif", "end", "false", "for", "function", "goto", "if", "in", "local", "nil", "not", "or", "repeat", "return", "then", "true", "until", "while"],
        Lang::Ruby => &["alias", "and", "begin", "break", "case", "class", "def", "defined?", "do", "else", "elsif", "end", "ensure", "false", "for", "if", "in", "module", "next", "nil", "not", "or", "redo", "rescue", "retry", "return", "self", "super", "then", "true", "undef", "unless", "until", "when", "while", "yield", "require", "puts"],
        Lang::Json | Lang::Yaml | Lang::Toml => &["true", "false", "null", "yes", "no", "on", "off", "True", "False", "Null", "~"],
        _ => &[],
    }
}

fn is_ident_start(c: u16) -> bool {
    c == b'_' as u16 || (c as u8 as u16 == c && (c as u8).is_ascii_alphabetic()) || c >= 0x80
}
fn is_ident(c: u16) -> bool {
    is_ident_start(c) || (c < 0x80 && (c as u8).is_ascii_digit())
}
fn ch(c: u16) -> char {
    char::from_u32(c as u32).unwrap_or('\0')
}

struct Lx<'a> {
    t: &'a [u16],
    i: usize,
    out: Vec<Span>,
}

impl<'a> Lx<'a> {
    fn at(&self, k: usize) -> u16 {
        *self.t.get(self.i + k).unwrap_or(&0)
    }
    fn starts(&self, s: &str) -> bool {
        s.encode_utf16().enumerate().all(|(k, u)| self.at(k) == u)
    }
    fn starts_ci(&self, s: &str) -> bool {
        s.chars().enumerate().all(|(k, u)| ch(self.at(k)).to_ascii_lowercase() == u)
    }
    fn push(&mut self, start: usize, tok: Tok) {
        if self.i > start {
            self.out.push(Span { start, len: self.i - start, tok });
        }
    }
    fn to_eol(&mut self) {
        while self.i < self.t.len() && self.t[self.i] != b'\n' as u16 && self.t[self.i] != b'\r' as u16 {
            self.i += 1;
        }
    }
    fn until(&mut self, end: &str) {
        while self.i < self.t.len() && !self.starts(end) {
            self.i += 1;
        }
        self.i = (self.i + end.encode_utf16().count()).min(self.t.len());
    }
    fn string(&mut self, q: u16, escapes: bool, multiline: bool) {
        self.i += 1;
        while self.i < self.t.len() {
            let c = self.t[self.i];
            if escapes && c == b'\\' as u16 {
                self.i += 2;
                continue;
            }
            self.i += 1;
            if c == q {
                break;
            }
            if !multiline && c == b'\n' as u16 {
                break;
            }
        }
        self.i = self.i.min(self.t.len());
    }
    fn number(&mut self) {
        let s = self.i;
        if self.starts("0x") || self.starts("0X") || self.starts("0b") || self.starts("0B") {
            self.i += 2;
        }
        while self.i < self.t.len() {
            let c = self.t[self.i];
            let cc = ch(c);
            if cc.is_ascii_alphanumeric() || cc == '_' || (cc == '.' && ch(self.at(1)).is_ascii_digit()) {
                self.i += 1;
            } else if (cc == '+' || cc == '-') && matches!(ch(self.t[self.i - 1]), 'e' | 'E') && !self.starts_hex(s) {
                self.i += 1;
            } else {
                break;
            }
        }
        self.push(s, Tok::Num);
    }
    fn starts_hex(&self, s: usize) -> bool {
        self.t.len() > s + 1 && self.t[s] == b'0' as u16 && matches!(ch(self.t[s + 1]), 'x' | 'X')
    }
    fn word(&mut self) -> String {
        let s = self.i;
        while self.i < self.t.len() && is_ident(self.t[self.i]) {
            self.i += 1;
        }
        String::from_utf16_lossy(&self.t[s..self.i])
    }
    fn next_nonspace(&self) -> u16 {
        let mut k = self.i;
        while k < self.t.len() && matches!(ch(self.t[k]), ' ' | '\t') {
            k += 1;
        }
        *self.t.get(k).unwrap_or(&0)
    }
    fn line_start(&self) -> bool {
        let mut k = self.i;
        while k > 0 {
            let c = ch(self.t[k - 1]);
            if c == '\n' || c == '\r' {
                return true;
            }
            if c != ' ' && c != '\t' {
                return false;
            }
            k -= 1;
        }
        true
    }
}

/// Подсветка: список отрезков (UTF-16) с типами.
pub fn highlight(t: &[u16], lang: Lang) -> Vec<Span> {
    let mut x = Lx { t, i: 0, out: vec![] };
    match lang {
        Lang::Plain => {}
        Lang::Xml => xml(&mut x),
        Lang::Markdown => markdown(&mut x),
        _ => code(&mut x, lang),
    }
    x.out
}

fn code(x: &mut Lx, lang: Lang) {
    let kw = keywords(lang);
    let ci = matches!(lang, Lang::Sql | Lang::Bat | Lang::PowerShell);
    let slash_comments = matches!(lang, Lang::C | Lang::CSharp | Lang::Java | Lang::Js | Lang::Rust | Lang::Go | Lang::Kotlin | Lang::Swift | Lang::Dart | Lang::Php | Lang::Css | Lang::Json);
    let hash_comments = matches!(lang, Lang::Python | Lang::Shell | Lang::PowerShell | Lang::Ruby | Lang::Yaml | Lang::Toml | Lang::Ini | Lang::Php);
    let types_upper = matches!(lang, Lang::Rust | Lang::CSharp | Lang::Java | Lang::Js | Lang::Kotlin | Lang::Swift | Lang::Dart | Lang::Go);
    while x.i < x.t.len() {
        let s = x.i;
        let c = ch(x.t[x.i]);
        // комментарии
        if slash_comments && x.starts("//") {
            x.to_eol();
            x.push(s, Tok::Comment);
            continue;
        }
        if (slash_comments || lang == Lang::Sql) && x.starts("/*") {
            x.i += 2;
            x.until("*/");
            x.push(s, Tok::Comment);
            continue;
        }
        if lang == Lang::PowerShell && x.starts("<#") {
            x.until("#>");
            x.push(s, Tok::Comment);
            continue;
        }
        if hash_comments && c == '#' && !(lang == Lang::Shell && x.starts("#{")) {
            x.to_eol();
            x.push(s, Tok::Comment);
            continue;
        }
        if lang == Lang::Ini && c == ';' && x.line_start() {
            x.to_eol();
            x.push(s, Tok::Comment);
            continue;
        }
        if (lang == Lang::Sql || lang == Lang::Lua) && x.starts("--") {
            if lang == Lang::Lua && x.starts("--[[") {
                x.until("]]");
            } else {
                x.to_eol();
            }
            x.push(s, Tok::Comment);
            continue;
        }
        if lang == Lang::Bat && x.line_start() && (x.starts_ci("rem ") || x.starts_ci("rem\r") || x.starts_ci("rem\n") || x.starts("::")) {
            x.to_eol();
            x.push(s, Tok::Comment);
            continue;
        }
        if lang == Lang::C && c == '#' && x.line_start() {
            x.i += 1;
            x.word();
            x.push(s, Tok::Kw);
            continue;
        }
        // секции INI/TOML
        if matches!(lang, Lang::Ini | Lang::Toml) && c == '[' && x.line_start() {
            x.to_eol();
            x.push(s, Tok::Tag);
            continue;
        }
        if lang == Lang::Bat && c == ':' && x.line_start() {
            x.to_eol();
            x.push(s, Tok::Tag);
            continue;
        }
        // строки
        if c == '"' || c == '\'' || (c == '`' && matches!(lang, Lang::Js | Lang::Shell | Lang::Go)) {
            if lang == Lang::Python && (x.starts("\"\"\"") || x.starts("'''")) {
                let q = if c == '"' { "\"\"\"" } else { "'''" };
                x.i += 3;
                x.until(q);
                x.push(s, Tok::Str);
                continue;
            }
            if c == '\'' && matches!(lang, Lang::Rust) {
                // 'a' — символ, а 'a без закрывающей — время жизни
                let closes = (1..=4).any(|k| x.at(k) == b'\'' as u16 && x.at(k - 1) != b'\\' as u16 || (k == 3 && x.at(1) == b'\\' as u16 && x.at(3) == b'\'' as u16));
                if !closes {
                    x.i += 1;
                    x.word();
                    x.push(s, Tok::Type);
                    continue;
                }
            }
            if c == '\'' && matches!(lang, Lang::Ini | Lang::Bat) {
                x.i += 1;
                continue;
            }
            let raw = matches!(lang, Lang::PowerShell | Lang::Sql) && c == '\'';
            x.string(x.t[x.i], !raw && lang != Lang::Bat, c == '`' || lang == Lang::Toml);
            // ключ JSON: строка, за которой двоеточие
            let tok = if lang == Lang::Json && x.next_nonspace() == b':' as u16 { Tok::Key } else { Tok::Str };
            x.push(s, tok);
            continue;
        }
        // числа
        if c.is_ascii_digit() && (s == 0 || !is_ident(x.t[s - 1])) {
            x.number();
            continue;
        }
        // переменные $x, %x%, @декоратор
        if (c == '$' && matches!(lang, Lang::Shell | Lang::PowerShell | Lang::Php)) || (c == '@' && matches!(lang, Lang::Python | Lang::Java | Lang::Kotlin | Lang::CSharp | Lang::Css | Lang::Dart | Lang::Swift)) {
            x.i += 1;
            if x.i < x.t.len() && x.t[x.i] == b'{' as u16 {
                x.until("}");
            } else {
                x.word();
            }
            x.push(s, if c == '@' { Tok::Kw } else { Tok::Type });
            continue;
        }
        if c == '%' && lang == Lang::Bat {
            x.i += 1;
            while x.i < x.t.len() && x.t[x.i] != b'%' as u16 && x.t[x.i] != b'\n' as u16 {
                x.i += 1;
            }
            x.i = (x.i + 1).min(x.t.len());
            x.push(s, Tok::Type);
            continue;
        }
        if c == '#' && lang == Lang::Css {
            x.i += 1;
            x.word();
            x.push(s, Tok::Num);
            continue;
        }
        // слова
        if is_ident_start(x.t[x.i]) {
            let w = x.word();
            let is_kw = if ci { kw.iter().any(|k| k.eq_ignore_ascii_case(&w)) } else { kw.contains(&w.as_str()) };
            let next = x.next_nonspace();
            let tok = if matches!(lang, Lang::Ini | Lang::Toml | Lang::Yaml | Lang::Css) && (next == b'=' as u16 || next == b':' as u16) && (lang == Lang::Css || x.line_start_of(s)) {
                Some(Tok::Key)
            } else if is_kw {
                Some(Tok::Kw)
            } else if types_upper && w.chars().next().map(|c| c.is_ascii_uppercase()).unwrap_or(false) {
                Some(Tok::Type)
            } else if lang == Lang::Rust && next == b'!' as u16 {
                Some(Tok::Kw)
            } else {
                None
            };
            if let Some(t) = tok {
                x.push(s, t);
            }
            continue;
        }
        x.i += 1;
    }
}

impl Lx<'_> {
    fn line_start_of(&self, pos: usize) -> bool {
        let mut k = pos;
        while k > 0 {
            let c = ch(self.t[k - 1]);
            if c == '\n' || c == '\r' {
                return true;
            }
            if !matches!(c, ' ' | '\t' | '-') {
                return false;
            }
            k -= 1;
        }
        true
    }
}

fn xml(x: &mut Lx) {
    while x.i < x.t.len() {
        let s = x.i;
        if x.starts("<!--") {
            x.until("-->");
            x.push(s, Tok::Comment);
            continue;
        }
        if x.starts("<![CDATA[") {
            x.until("]]>");
            x.push(s, Tok::Str);
            continue;
        }
        if x.at(0) == b'<' as u16 {
            x.i += 1;
            while x.i < x.t.len() && matches!(ch(x.t[x.i]), '/' | '?' | '!') {
                x.i += 1;
            }
            while x.i < x.t.len() && (is_ident(x.t[x.i]) || matches!(ch(x.t[x.i]), ':' | '-' | '.')) {
                x.i += 1;
            }
            x.push(s, Tok::Tag);
            // атрибуты до '>'
            while x.i < x.t.len() && x.t[x.i] != b'>' as u16 {
                let a = x.i;
                let c = ch(x.t[x.i]);
                if c == '"' || c == '\'' {
                    x.string(x.t[x.i], false, true);
                    x.push(a, Tok::Str);
                } else if is_ident_start(x.t[x.i]) {
                    while x.i < x.t.len() && (is_ident(x.t[x.i]) || matches!(ch(x.t[x.i]), ':' | '-' | '.')) {
                        x.i += 1;
                    }
                    x.push(a, Tok::Attr);
                } else if c == '/' || c == '?' {
                    x.i += 1;
                    x.push(a, Tok::Tag);
                } else {
                    x.i += 1;
                }
            }
            if x.i < x.t.len() {
                let a = x.i;
                x.i += 1;
                x.push(a, Tok::Tag);
            }
            continue;
        }
        if x.at(0) == b'&' as u16 {
            x.i += 1;
            while x.i < x.t.len() && x.t[x.i] != b';' as u16 && x.i - s < 12 && x.t[x.i] != b' ' as u16 {
                x.i += 1;
            }
            x.i = (x.i + 1).min(x.t.len());
            x.push(s, Tok::Num);
            continue;
        }
        x.i += 1;
    }
}

fn markdown(x: &mut Lx) {
    while x.i < x.t.len() {
        let s = x.i;
        let c = ch(x.t[x.i]);
        if c == '#' && x.line_start() {
            x.to_eol();
            x.push(s, Tok::Kw);
            continue;
        }
        if x.starts("```") {
            x.i += 3;
            x.until("```");
            x.push(s, Tok::Str);
            continue;
        }
        if c == '`' {
            x.string(b'`' as u16, false, false);
            x.push(s, Tok::Str);
            continue;
        }
        if x.starts("**") {
            x.i += 2;
            x.until("**");
            x.push(s, Tok::Type);
            continue;
        }
        if (c == '-' || c == '*' || c == '>') && x.line_start() && x.at(1) == b' ' as u16 {
            x.i += 1;
            x.push(s, Tok::Tag);
            continue;
        }
        if c == '[' {
            x.until("]");
            if x.at(0) == b'(' as u16 {
                x.until(")");
            }
            x.push(s, Tok::Attr);
            continue;
        }
        x.i += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hl(src: &str, l: Lang) -> Vec<(String, Tok)> {
        let u: Vec<u16> = src.encode_utf16().collect();
        highlight(&u, l).into_iter().map(|s| (String::from_utf16_lossy(&u[s.start..s.start + s.len]), s.tok)).collect()
    }

    #[test]
    fn rust() {
        let v = hl("fn main() { let s = \"при\\\"вет\"; // комм\n let x = 0x1F + 2.5e-3; String::new(); 'a'; &'b str }", Lang::Rust);
        assert!(v.contains(&("fn".into(), Tok::Kw)));
        assert!(v.contains(&("\"при\\\"вет\"".into(), Tok::Str)));
        assert!(v.contains(&("// комм".into(), Tok::Comment)));
        assert!(v.contains(&("0x1F".into(), Tok::Num)));
        assert!(v.contains(&("2.5e-3".into(), Tok::Num)));
        assert!(v.contains(&("String".into(), Tok::Type)));
        assert!(v.contains(&("'a'".into(), Tok::Str)));
        assert!(v.contains(&("'b".into(), Tok::Type)), "время жизни");
    }

    #[test]
    fn python_and_json() {
        let v = hl("@dec\ndef f(x):\n    \"\"\"doc\n    \"\"\"\n    return x  # c", Lang::Python);
        assert!(v.contains(&("@dec".into(), Tok::Kw)));
        assert!(v.contains(&("\"\"\"doc\n    \"\"\"".into(), Tok::Str)));
        assert!(v.contains(&("# c".into(), Tok::Comment)));
        let v = hl("{\"key\": \"val\", \"n\": -1, \"b\": true}", Lang::Json);
        assert_eq!(v[0], ("\"key\"".into(), Tok::Key));
        assert_eq!(v[1], ("\"val\"".into(), Tok::Str));
        assert!(v.contains(&("true".into(), Tok::Kw)));
        assert!(v.contains(&("1".into(), Tok::Num)));
    }

    #[test]
    fn xml_and_ini() {
        let v = hl("<!-- c --><a href=\"x\">t &amp; u</a>", Lang::Xml);
        assert_eq!(v[0].1, Tok::Comment);
        assert!(v.contains(&("<a".into(), Tok::Tag)));
        assert!(v.contains(&("href".into(), Tok::Attr)));
        assert!(v.contains(&("\"x\"".into(), Tok::Str)));
        assert!(v.contains(&("&amp;".into(), Tok::Num)));
        assert!(v.contains(&("</a".into(), Tok::Tag)));
        let v = hl("; comment\n[section]\nkey = value\n", Lang::Ini);
        assert_eq!(v[0].1, Tok::Comment);
        assert_eq!(v[1], ("[section]".into(), Tok::Tag));
        assert_eq!(v[2], ("key".into(), Tok::Key));
    }

    #[test]
    fn sql_bat_yaml() {
        let v = hl("SELECT name FROM t -- x", Lang::Sql);
        assert!(v.contains(&("SELECT".into(), Tok::Kw)));
        assert!(v.contains(&("-- x".into(), Tok::Comment)));
        let v = hl("@echo off\nREM коммент\nset X=%PATH%\n:label", Lang::Bat);
        assert!(v.contains(&("REM коммент".into(), Tok::Comment)));
        assert!(v.contains(&("%PATH%".into(), Tok::Type)));
        assert!(v.contains(&(":label".into(), Tok::Tag)));
        let v = hl("name: app\nitems:\n  - port: 80 # c", Lang::Yaml);
        assert_eq!(v[0], ("name".into(), Tok::Key));
        assert!(v.contains(&("port".into(), Tok::Key)));
        assert!(v.contains(&("80".into(), Tok::Num)));
    }

    #[test]
    fn langs() {
        assert_eq!(lang_for("rs", ""), Lang::Rust);
        assert_eq!(lang_for("", "makefile"), Lang::Shell);
        assert_eq!(lang_for("txt", ""), Lang::Plain);
    }
}
