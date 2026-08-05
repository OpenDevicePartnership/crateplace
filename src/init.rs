use crate::config::{ByteFormat, ByteUnit, UnitParseError};
use crate::file_error::FileError;
use std::str::FromStr;
use toml_edit::{DocumentMut, Item as TomlItem, Table, Value};

use syn::Item;

#[derive(Debug, thiserror::Error)]
pub enum InitError {
    #[error("invalid manifest path")]
    InvalidManifestPath,
    #[error("failed to find manifest")]
    ManifestNotFound,
    #[error("file error")]
    FileError(
        #[source]
        #[from]
        FileError,
    ),
    #[error("failed to parse \"memory.x\"")]
    MemoryXParseError(
        #[source]
        #[from]
        MemoryXParseError,
    ),
    #[error("Failed to modify \"build.rs\"")]
    FailedToModifyBuildRs,
    #[error("Failed to parse Cargo.toml")]
    TomlError(
        #[source]
        #[from]
        toml_edit::TomlError,
    ),
    #[error("Build dependencies not a table")]
    DepsNotTableError,
}

pub fn generate_memory_toml(memx: &MemoryX) -> String {
    let mut res = String::new();
    res += &format!(
        "ram = {{ origin = \"{}\", length = \"{}\" }}\n\n[sections]\n",
        memx.ram.origin, memx.ram.length
    );
    res += &format!(
        "flash = {{ origin = \"{}\", length = \"{}\", priority = 1 }}\n",
        memx.flash.origin, memx.flash.length
    );
    for (i, section) in memx.sections.iter().enumerate() {
        res += &format!(
            "{} = {{ origin = \"{}\", length = \"{}\", priority = {} }}\n",
            section.name.to_lowercase(),
            section.origin,
            section.length,
            i + 2
        );
    }
    res
}

fn generate_build_rs_code(memory_x: &MemoryX) -> String {
    format!(
        "\n    if let Err(err) = crateplace::CratePlacer::new(){}{}.buildscript() {{
        crateplace::report(&err);
        std::process::exit(1);
    }}",
        if memory_x.pre.is_some() {
            ".pre_script(\"pre.x\")"
        } else {
            ""
        },
        if memory_x.post.is_some() {
            ".post_script(\"post.x\")"
        } else {
            ""
        }
    )
}

pub fn init_cargo_toml(cargo_toml: &str) -> Result<String, InitError> {
    let mut doc = cargo_toml.parse::<DocumentMut>()?;

    let build_deps = doc
        .entry("build-dependencies")
        .or_insert_with(|| TomlItem::Table(Table::new()))
        .as_table_mut()
        .ok_or(InitError::DepsNotTableError)?;

    if !build_deps.get("crateplace").is_some() {
        let mut inline = toml_edit::InlineTable::new();
        inline.insert(
            "git",
            "https://github.com/OpenDevicePartnership/crateplace.git".into(),
        );
        build_deps.insert("crateplace", TomlItem::Value(Value::InlineTable(inline)));
    }

    Ok(doc.to_string())
}

pub fn init_existing_build_rs(
    memory_x: &MemoryX,
    build_script: String,
) -> Result<String, InitError> {
    let build_rs_content = generate_build_rs_code(memory_x);

    if build_script.contains("crateplace::CratePlacer::new()") {
        return Ok(build_script);
    }

    let parsed = syn::parse_file(&build_script).map_err(|_| InitError::FailedToModifyBuildRs)?;
    let main_fn = parsed
        .items
        .iter()
        .find_map(|item| match item {
            Item::Fn(f) if f.sig.ident == "main" => Some(f),
            _ => None,
        })
        .ok_or(InitError::FailedToModifyBuildRs)?;

    let close_span = main_fn.block.brace_token.span.close();
    let insert_at = close_span.byte_range().end - 1;

    let mut res = String::new();
    res.push_str(&build_script[..insert_at]);
    res.push_str("\n    // Added by crateplace");
    res.push_str(&build_rs_content);
    res.push('\n');
    res.push_str(&build_script[insert_at..]);
    Ok(res)
}

pub fn init_new_build_rs(memory_x: &MemoryX) -> String {
    format!(
        "// Added by crateplace\nfn main() {{{}\n}}",
        generate_build_rs_code(memory_x)
    )
}

#[derive(Debug)]
pub struct MemoryXSection {
    name: String,
    origin: ByteUnit,
    length: ByteUnit,
}

#[derive(Debug)]
pub struct MemoryX {
    ram: MemoryXSection,
    flash: MemoryXSection,
    pre: Option<String>,
    sections: Vec<MemoryXSection>,
    post: Option<String>,
}

impl Default for MemoryX {
    fn default() -> Self {
        Self {
            ram: MemoryXSection {
                name: "ram".to_string(),
                origin: ByteUnit::new(0x20000000, ByteFormat::Hex),
                length: ByteUnit::new(0x20000, ByteFormat::Kibi),
            },
            flash: MemoryXSection {
                name: "flash".to_string(),
                origin: ByteUnit::new(0, ByteFormat::Hex),
                length: ByteUnit::new(1024, ByteFormat::Mebi),
            },
            pre: None,
            sections: vec![],
            post: None,
        }
    }
}

impl MemoryX {
    pub fn get_pre(&self) -> Option<&str> {
        self.pre.as_deref()
    }

    pub fn get_post(&self) -> Option<&str> {
        self.post.as_deref()
    }
}

#[derive(Debug)]
struct TearText {
    text: Vec<char>,
    pos: usize,
    line_no: usize,
}

impl TearText {
    pub fn new(text: &str) -> Self {
        Self {
            text: text.chars().collect(),
            pos: 0,
            line_no: 1,
        }
    }

    pub fn current(&self) -> Option<char> {
        self.text.get(self.pos).copied()
    }

    pub fn next(&self) -> Option<char> {
        self.text.get(self.pos + 1).copied()
    }

    pub fn previous(&self) -> Option<char> {
        self.pos
            .checked_sub(1)
            .and_then(|i| self.text.get(i))
            .copied()
    }

    pub fn advance(&mut self) -> Option<char> {
        self.pos += 1;
        let character = self.text.get(self.pos).copied();
        if character == Some('\n') {
            self.line_no += 1;
        }
        character
    }

    pub fn line_number(&self) -> usize {
        self.line_no
    }

    pub fn matches_word(&self, word: &str) -> bool {
        self.text
            .get(self.pos..self.pos + word.chars().count())
            .is_some_and(|text| text.iter().zip(word.chars()).all(|(a, b)| *a == b))
    }
}

#[derive(Debug, Copy, Clone)]
enum Part {
    Pre,
    Mem,
    Post,
}

#[derive(Debug)]
struct PartState {
    current: Part,
    next: Option<Part>,
}

impl PartState {
    pub fn new() -> Self {
        Self {
            current: Part::Pre,
            next: None,
        }
    }

    pub fn next(&mut self, text: &TearText, comment: Comment) -> Part {
        if let Some(next) = self.next.take() {
            self.current = next;
        }
        match (self.current, comment, text.current()) {
            (Part::Pre, Comment::No, Some('M')) if text.matches_word("MEMORY") => {
                self.current = Part::Mem;
            }
            (Part::Mem, Comment::No, Some('}')) => {
                self.next = Some(Part::Post);
            }
            _ => (),
        }
        self.current
    }
}

#[derive(Debug, Copy, Clone)]
enum Comment {
    In(usize),
    InLine,
    No,
}

#[derive(Debug)]
struct CommentState {
    current: Comment,
    next: Option<Comment>,
}

impl CommentState {
    pub fn new() -> Self {
        Self {
            current: Comment::No,
            next: None,
        }
    }

    pub fn next(&mut self, text: &TearText) -> Comment {
        if let Some(next) = self.next.take() {
            self.current = next;
        }
        match (&self.current, text.previous(), text.current(), text.next()) {
            (Comment::No, _, Some('/'), Some('*')) => {
                self.current = Comment::In(text.line_number());
            }
            (Comment::In(_), Some('*'), Some('/'), _) => {
                self.next = Some(Comment::No);
            }
            (Comment::No, _, Some('/'), Some('/')) => {
                self.current = Comment::InLine;
            }
            (Comment::InLine, _, Some('\n'), _) => {
                self.current = Comment::No;
            }
            _ => (),
        }
        self.current
    }

    pub fn current(&self) -> Comment {
        self.current
    }
}

fn base_parse(s: &str) -> Result<(Option<String>, String, Option<String>), MemoryXParseError> {
    let (mut pre, mut memory, mut post) = (String::new(), String::new(), String::new());
    let mut text = TearText::new(s);
    let mut comment_state = CommentState::new();
    let mut part_state = PartState::new();

    let mut pre_contains_section = false;
    let mut post_contains_section = false;

    while let Some(current) = text.current() {
        let comment = comment_state.next(&text);
        let text_part = part_state.next(&text, comment);

        match (&text_part, &comment) {
            (Part::Pre, Comment::No) if !current.is_whitespace() => {
                pre_contains_section = true;
                pre.push(current);
            }
            (Part::Pre, _) => pre.push(current),
            (Part::Mem, Comment::No) => memory.push(current),
            (Part::Post, Comment::No) if !current.is_whitespace() => {
                post_contains_section = true;
                post.push(current);
            }
            (Part::Post, _) => post.push(current),
            _ => (),
        };
        text.advance();
    }
    match comment_state.current() {
        Comment::No | Comment::InLine => Ok((
            pre_contains_section.then_some(pre.trim().to_string()),
            memory.trim().to_string(),
            post_contains_section.then_some(post.trim().to_string()),
        )),
        Comment::In(line_number) => Err(MemoryXParseError::UnterminatedComment(line_number)),
    }
}

#[derive(thiserror::Error, Debug)]
pub enum MemoryXParseError {
    #[error("comment in memory.x not terminated, comment starts at: line {0}")]
    UnterminatedComment(usize),
    #[error("failed to parse memory sections")]
    FailedToParseMemorySections,
    #[error("crateplace does not support section attibutes, example: (rx)")]
    NoSupportAttr,
    #[error("failed to parse number in section")]
    UnitParseError(
        #[source]
        #[from]
        UnitParseError,
    ),
    #[error("failed to parse section, expected: {0}")]
    Expected(&'static str),
    #[error("missing section: {0}")]
    MissingSection(&'static str),
}

fn parse_memory_content(text: &str) -> Result<Vec<MemoryXSection>, MemoryXParseError> {
    let mut content = text
        .split_once('{')
        .map(|(_, p)| p.trim_end_matches('}').split_whitespace().peekable())
        .ok_or(MemoryXParseError::FailedToParseMemorySections)?;
    let mut res = Vec::new();
    while content.peek().is_some() {
        let name = content
            .next()
            .ok_or(MemoryXParseError::Expected("name"))?
            .to_string();
        let next = content
            .next()
            .ok_or(MemoryXParseError::Expected("\":\" after name"))?;
        let next_char = next.chars().next();
        match next_char {
            Some('(') => {
                return Err(MemoryXParseError::NoSupportAttr);
            }
            Some(':') => (),
            _ => {
                return Err(MemoryXParseError::Expected("\":\" after name"));
            }
        };
        let next = content.next();
        if next != Some("ORIGIN") && next != Some("org") {
            Err(MemoryXParseError::Expected("\"ORIGIN\" after \":\""))?
        }
        if content.next() != Some("=") {
            Err(MemoryXParseError::Expected("\"= after \"ORIGIN\""))?
        }
        let origin = ByteUnit::from_str(
            content
                .next()
                .ok_or(MemoryXParseError::Expected("number after \"=\""))?
                .trim_end_matches(','),
        )?;
        let next = content.next();
        if next != Some("LENGTH") && next != Some("l") {
            Err(MemoryXParseError::Expected("\"LENGTH\" after \"ORIGIN\""))?
        }
        if content.next() != Some("=") {
            Err(MemoryXParseError::Expected("\"=\" after \"LENGTH\""))?
        }
        let length = ByteUnit::from_str(
            content
                .next()
                .ok_or(MemoryXParseError::Expected("number after \"=\""))?
                .trim_end_matches(','),
        )?;
        res.push(MemoryXSection {
            origin,
            length,
            name,
        });
    }
    Ok(res)
}

impl FromStr for MemoryX {
    type Err = MemoryXParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (pre, memory, post) = base_parse(s)?;
        let mut sections = parse_memory_content(&memory)?;
        let ram = sections
            .iter()
            .position(|s| s.name == "RAM")
            .map(|i| sections.remove(i))
            .ok_or(MemoryXParseError::MissingSection("RAM"))?;
        let flash = sections
            .iter()
            .position(|s| s.name == "FLASH")
            .map(|i| sections.remove(i))
            .ok_or(MemoryXParseError::MissingSection("FLASH"))?;
        Ok(Self {
            pre,
            sections,
            post,
            ram,
            flash,
        })
    }
}
