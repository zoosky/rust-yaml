//! YAML parser for converting tokens to events

use crate::{
    error::ErrorContext, tag::TagResolver, BasicScanner, Error, Limits, Position, Result, Scanner,
    Token, TokenType,
};

pub mod events;
pub mod streaming;
// pub mod optimizations; // Temporarily disabled
pub use events::*;
pub use streaming::*;
// pub use optimizations::*;

/// Trait for YAML parsers that convert token streams to events
pub trait Parser {
    /// Check if there are more events available
    fn check_event(&self) -> bool;

    /// Peek at the next event without consuming it
    fn peek_event(&self) -> Result<Option<&Event>>;

    /// Get the next event, consuming it
    fn get_event(&mut self) -> Result<Option<Event>>;

    /// Reset the parser state
    fn reset(&mut self);

    /// Get the current position in the input
    fn position(&self) -> Position;
}

/// Walks back through `events` looking for an unclosed `DocumentStart`
/// (i.e. one without a matching `DocumentEnd` after it). Returns true if
/// the parser is still inside a document.
fn has_open_document(events: &[Event]) -> bool {
    for ev in events.iter().rev() {
        match &ev.event_type {
            EventType::DocumentEnd { .. } => return false,
            EventType::DocumentStart { .. } => return true,
            _ => {}
        }
    }
    false
}

/// Emit `MappingEnd` / `SequenceEnd` events to close any unbalanced
/// collection starts in `events`. Called before emitting a `DocumentEnd`
/// when an outer construct (e.g. a new `---` marker) forces the previous
/// document closed without going through the usual indent-driven
/// `BlockEnd` token path. Also synthesises implicit empty scalars for
/// mappings that have an odd child-event count (i.e. a key without a
/// value before the close).
/// Return true when there is at least one *flow* collection still open
/// (a `SequenceStart` / `MappingStart` with `flow_style=true` without a
/// matching `…End` afterwards). Used at end-of-stream to enforce §7.4.
fn has_unclosed_flow_collection(events: &[Event]) -> bool {
    let mut depth: i32 = 0;
    for ev in events.iter() {
        match &ev.event_type {
            EventType::SequenceStart { flow_style: true, .. }
            | EventType::MappingStart { flow_style: true, .. } => depth += 1,
            EventType::SequenceEnd | EventType::MappingEnd => {
                if depth > 0 {
                    depth -= 1;
                }
            }
            _ => {}
        }
    }
    depth > 0
}

/// Walk `events` to detect a document that already contains a closed
/// root-level node. Returns true when the second root node arrives and
/// the existing event stack has no unmatched MapStart/SeqStart.
fn second_root_node_present(events: &[Event]) -> bool {
    let mut after_doc_start = false;
    let mut has_root_node = false;
    let mut depth = 0i32;
    for e in events.iter() {
        match &e.event_type {
            EventType::DocumentStart { .. } => {
                after_doc_start = true;
                has_root_node = false;
                depth = 0;
            }
            EventType::DocumentEnd { .. } => after_doc_start = false,
            EventType::MappingStart { .. } | EventType::SequenceStart { .. } => depth += 1,
            EventType::MappingEnd | EventType::SequenceEnd => {
                depth -= 1;
                if depth == 0 {
                    has_root_node = true;
                }
            }
            EventType::Scalar { .. } | EventType::Alias { .. } => {
                if depth == 0 {
                    has_root_node = true;
                }
            }
            _ => {}
        }
    }
    after_doc_start && has_root_node && depth == 0
}

/// Return true when the innermost still-open mapping has an odd number
/// of children — i.e. a key has been emitted but its value has not.
/// Used to decide when to synthesise an implicit empty scalar.
fn innermost_mapping_has_odd_children(events: &[Event]) -> bool {
    let mut stack: Vec<(&'static str, usize)> = Vec::new();
    for ev in events.iter() {
        match &ev.event_type {
            EventType::DocumentStart { .. } | EventType::DocumentEnd { .. } => {
                stack.clear();
            }
            EventType::MappingStart { .. } => stack.push(("map", 0)),
            EventType::SequenceStart { .. } => stack.push(("seq", 0)),
            EventType::MappingEnd | EventType::SequenceEnd => {
                stack.pop();
                if let Some(parent) = stack.last_mut() {
                    parent.1 += 1;
                }
            }
            EventType::Scalar { .. } | EventType::Alias { .. } => {
                if let Some(parent) = stack.last_mut() {
                    parent.1 += 1;
                }
            }
            _ => {}
        }
    }
    matches!(stack.last(), Some(("map", n)) if n % 2 == 1)
}

fn close_open_collections(events: &mut Vec<Event>, pos: Position) {
    // Each entry: (kind, children_at_this_depth) where `kind` is "map"
    // or "seq". `children` counts top-level node events emitted inside
    // this collection (Scalar, Alias, or a closed nested collection).
    let mut stack: Vec<(&'static str, usize)> = Vec::new();
    for ev in events.iter() {
        match &ev.event_type {
            EventType::DocumentStart { .. } | EventType::DocumentEnd { .. } => {
                stack.clear();
            }
            EventType::MappingStart { .. } => stack.push(("map", 0)),
            EventType::SequenceStart { .. } => stack.push(("seq", 0)),
            EventType::MappingEnd | EventType::SequenceEnd => {
                stack.pop();
                if let Some(parent) = stack.last_mut() {
                    parent.1 += 1;
                }
            }
            EventType::Scalar { .. } | EventType::Alias { .. } => {
                if let Some(parent) = stack.last_mut() {
                    parent.1 += 1;
                }
            }
            _ => {}
        }
    }
    while let Some((kind, children)) = stack.pop() {
        if kind == "map" && children % 2 == 1 {
            // Odd child count → last key has no value yet. Spec says
            // emit implicit empty scalar (YAML 1.2 §6.9.1).
            events.push(Event::scalar(
                pos,
                None,
                None,
                String::new(),
                true,
                false,
                ScalarStyle::Plain,
            ));
        }
        match kind {
            "map" => events.push(Event::mapping_end(pos)),
            "seq" => events.push(Event::sequence_end(pos)),
            _ => {}
        }
    }
}

/// Basic parser implementation that converts tokens to events
#[derive(Debug)]
pub struct BasicParser {
    scanner: BasicScanner,
    events: Vec<Event>,
    event_index: usize,
    state: ParserState,
    state_stack: Vec<ParserState>,
    position: Position,
    pending_anchor: Option<String>,
    pending_tag: Option<String>,
    last_token_type: Option<TokenType>,
    scanning_error: Option<Error>,
    yaml_version: Option<(u8, u8)>,
    tag_directives: Vec<(String, String)>,
    tag_resolver: TagResolver,
    /// Anchor names that have been defined so far in the stream. Used to
    /// validate that aliases (`*name`) reference a known anchor (YAML 1.2
    /// §6.9.2). Forward references are forbidden, and we never reset this
    /// set — once defined, an anchor remains referenceable for the rest of
    /// the parse, matching common loader semantics.
    defined_anchors: std::collections::HashSet<String>,
    /// Line of the most recent `:` Value token. Used by the
    /// BlockMappingValue heuristic to tell apart "same-line value
    /// scalar" (6M2F) from "next-line sibling key" (6KGN).
    last_value_token_line: Option<usize>,
    /// True while we're holding an explicit `?` key that has not yet
    /// received its `:`. Used at end-of-stream to distinguish a
    /// spec-legal `? key` with implicit empty value from a missing-`:`
    /// bare scalar (yaml-test-suite 7MNF).
    explicit_key_pending: bool,
    /// Counts implicit single-pair flow mappings still open. A `,` or
    /// `]` while this is > 0 closes the innermost implicit mapping
    /// before continuing the outer flow sequence (§7.5).
    implicit_flow_pair_depth: usize,
}

/// Parser state for tracking context
#[derive(Debug, Clone, Copy, PartialEq)]
#[allow(dead_code)]
enum ParserState {
    StreamStart,
    StreamEnd,
    ImplicitDocumentStart,
    DocumentStart,
    DocumentContent,
    DocumentEnd,
    BlockNode,
    BlockMapping,
    BlockMappingKey,
    BlockMappingValue,
    BlockSequence,
    FlowMapping,
    FlowMappingKey,
    FlowMappingValue,
    FlowSequence,
    BlockEnd,
}

impl BasicParser {
    /// Create a new streaming parser (lazy parsing)
    pub fn new(input: String) -> Self {
        Self::with_limits(input, Limits::default())
    }

    /// Create a new streaming parser with custom limits
    pub fn with_limits(input: String, limits: Limits) -> Self {
        let scanner = BasicScanner::with_limits(input, limits);
        let position = scanner.position();

        Self {
            scanner,
            events: Vec::new(),
            event_index: 0,
            state: ParserState::StreamStart,
            state_stack: Vec::new(),
            position,
            pending_anchor: None,
            pending_tag: None,
            last_token_type: None,
            scanning_error: None,
            yaml_version: None,
            tag_directives: Vec::new(),
            tag_resolver: TagResolver::new(),
            defined_anchors: std::collections::HashSet::new(),
            last_value_token_line: None,
            explicit_key_pending: false,
            implicit_flow_pair_depth: 0,
        }
    }

    /// Create a new parser with eager parsing (for compatibility)
    pub fn new_eager(input: String) -> Self {
        Self::new_eager_with_limits(input, Limits::default())
    }

    /// Create a new parser with eager parsing and custom limits
    pub fn new_eager_with_limits(input: String, limits: Limits) -> Self {
        let mut scanner = BasicScanner::new_eager_with_limits(input, limits);
        let position = scanner.position();

        // Check if there were any scanning errors and store them
        let scanning_error = scanner.take_scanning_error();

        let mut parser = Self {
            scanner,
            events: Vec::new(),
            event_index: 0,
            state: ParserState::StreamStart,
            state_stack: Vec::new(),
            position,
            pending_anchor: None,
            pending_tag: None,
            last_token_type: None,
            scanning_error: None,
            yaml_version: None,
            tag_directives: Vec::new(),
            tag_resolver: TagResolver::new(),
            defined_anchors: std::collections::HashSet::new(),
            last_value_token_line: None,
            explicit_key_pending: false,
            implicit_flow_pair_depth: 0,
        };

        // If there was a scanning error, store it for later propagation.
        // Likewise, surface eager-parse errors via the same field so
        // `take_scanning_error` reports them.
        if let Some(error) = scanning_error {
            parser.scanning_error = Some(error);
        } else if let Err(error) = parser.parse_all() {
            parser.scanning_error = Some(error);
        }

        parser
    }

    /// Create parser from existing scanner
    pub fn from_scanner(scanner: BasicScanner) -> Self {
        let position = scanner.position();

        let mut parser = Self {
            scanner,
            events: Vec::new(),
            event_index: 0,
            state: ParserState::StreamStart,
            state_stack: Vec::new(),
            position,
            pending_anchor: None,
            pending_tag: None,
            last_token_type: None,
            scanning_error: None,
            yaml_version: None,
            tag_directives: Vec::new(),
            tag_resolver: TagResolver::new(),
            defined_anchors: std::collections::HashSet::new(),
            last_value_token_line: None,
            explicit_key_pending: false,
            implicit_flow_pair_depth: 0,
        };

        parser.parse_all().unwrap_or(());
        parser
    }

    /// Parse all tokens into events
    fn parse_all(&mut self) -> Result<()> {
        while self.scanner.check_token() {
            let token = match self.scanner.get_token()? {
                Some(token) => token,
                None => break,
            };

            self.position = token.end_position;
            self.process_token(token)?;
        }

        // Check for unclosed structures
        self.validate_final_state()?;

        // Ensure stream end
        if !self
            .events
            .iter()
            .any(|e| matches!(e.event_type, EventType::StreamEnd))
        {
            self.events.push(Event::stream_end(self.position));
        }

        Ok(())
    }

    /// YAML 1.2 §6.8: directives may appear only before the first
    /// document (`StreamStart` / `ImplicitDocumentStart`) or after an
    /// explicit `...` (`DocumentEnd`). Anywhere else they're invalid.
    fn check_directive_context(&self, pos: Position, name: &str) -> Result<()> {
        if matches!(
            self.state,
            ParserState::StreamStart
                | ParserState::ImplicitDocumentStart
                | ParserState::DocumentEnd
        ) {
            Ok(())
        } else {
            Err(Error::parse(
                pos,
                format!("{name} directive is only allowed before a document or after `...`"),
            ))
        }
    }

    /// Create implicit document start event with directives
    fn create_implicit_document_start(&mut self, position: Position) -> Event {
        let event = Event::document_start(
            position,
            self.yaml_version.take(),
            self.tag_directives.clone(),
            true,
        );
        self.tag_directives.clear();
        event
    }

    /// Validate that the parser is in a valid final state
    fn validate_final_state(&self) -> Result<()> {
        match self.state {
            ParserState::StreamEnd | ParserState::DocumentEnd | ParserState::DocumentContent => {
                // These are valid final states
                Ok(())
            }
            ParserState::BlockSequence | ParserState::FlowSequence => {
                let context = ErrorContext::from_input(self.scanner.input(), &self.position, 2)
                    .with_suggestion(
                        "Close the sequence with proper indentation or closing bracket".to_string(),
                    );
                Err(Error::unclosed_delimiter_with_context(
                    self.position,
                    self.position,
                    "sequence",
                    context,
                ))
            }
            ParserState::BlockMapping | ParserState::FlowMapping => {
                let context = ErrorContext::from_input(self.scanner.input(), &self.position, 2)
                    .with_suggestion(
                        "Close the mapping with proper indentation or closing brace".to_string(),
                    );
                Err(Error::unclosed_delimiter_with_context(
                    self.position,
                    self.position,
                    "mapping",
                    context,
                ))
            }
            _ => {
                let context = ErrorContext::from_input(self.scanner.input(), &self.position, 2)
                    .with_suggestion("Complete the YAML document structure".to_string());
                Err(Error::parse_with_context(
                    self.position,
                    format!("Document ended in unexpected state: {:?}", self.state),
                    context,
                ))
            }
        }
    }

    /// Generate the next event by processing the next token
    fn generate_next_event(&mut self) -> Result<()> {
        if let Some(token) = self.scanner.get_token()? {
            self.position = token.end_position;
            self.process_token(token)?;
        }
        Ok(())
    }

    /// Process a single token and generate appropriate events
    #[allow(clippy::cognitive_complexity)]
    fn process_token(&mut self, token: Token) -> Result<()> {
        // Store the token type for later use without cloning
        let token_type_for_tracking = match &token.token_type {
            TokenType::Scalar(..) => Some(TokenType::Scalar(
                String::new(),
                crate::scanner::QuoteStyle::Plain,
            )),
            TokenType::BlockScalarLiteral(..) => Some(TokenType::BlockScalarLiteral(String::new())),
            TokenType::BlockScalarFolded(..) => Some(TokenType::BlockScalarFolded(String::new())),
            TokenType::Alias(..) => Some(TokenType::Alias(String::new())),
            TokenType::Anchor(..) => Some(TokenType::Anchor(String::new())),
            TokenType::Tag(..) => Some(TokenType::Tag(String::new())),
            TokenType::Comment(..) => Some(TokenType::Comment(String::new())),
            other => {
                // For simple token types without data, we can safely clone
                match other {
                    TokenType::StreamStart => Some(TokenType::StreamStart),
                    TokenType::StreamEnd => Some(TokenType::StreamEnd),
                    TokenType::DocumentStart => Some(TokenType::DocumentStart),
                    TokenType::DocumentEnd => Some(TokenType::DocumentEnd),
                    TokenType::BlockSequenceStart => Some(TokenType::BlockSequenceStart),
                    TokenType::BlockMappingStart => Some(TokenType::BlockMappingStart),
                    TokenType::BlockEnd => Some(TokenType::BlockEnd),
                    TokenType::FlowSequenceStart => Some(TokenType::FlowSequenceStart),
                    TokenType::FlowSequenceEnd => Some(TokenType::FlowSequenceEnd),
                    TokenType::FlowMappingStart => Some(TokenType::FlowMappingStart),
                    TokenType::FlowMappingEnd => Some(TokenType::FlowMappingEnd),
                    TokenType::BlockEntry => Some(TokenType::BlockEntry),
                    TokenType::FlowEntry => Some(TokenType::FlowEntry),
                    TokenType::Key => Some(TokenType::Key),
                    TokenType::Value => Some(TokenType::Value),
                    TokenType::YamlDirective(_, _) => Some(TokenType::YamlDirective(0, 0)),
                    TokenType::TagDirective(_, _) => {
                        Some(TokenType::TagDirective(String::new(), String::new()))
                    }
                    _ => None,
                }
            }
        };

        match &token.token_type {
            TokenType::StreamStart => {
                self.events.push(Event::stream_start(token.start_position));
                self.state = ParserState::ImplicitDocumentStart;
            }

            TokenType::StreamEnd => {
                // YAML 1.2 §6.8: a directive must be followed by a
                // document body. If we reach end-of-stream with pending
                // `%YAML` / `%TAG` directives and no document was ever
                // opened, that's a parse error (yaml-test-suite 9MMA, B63P).
                if matches!(
                    self.state,
                    ParserState::ImplicitDocumentStart | ParserState::StreamStart
                ) && (self.yaml_version.is_some() || !self.tag_directives.is_empty())
                {
                    return Err(Error::parse(
                        token.start_position,
                        "Directive without a document body",
                    ));
                }
                // YAML 1.2 §7.4: every `[` / `{` must be closed before
                // end-of-stream. Walk the events; an unmatched
                // FlowSequenceStart / FlowMappingStart is invalid
                // (yaml-test-suite 6JTT, 9HCY, 9MQT/01).
                if has_unclosed_flow_collection(&self.events) {
                    return Err(Error::parse(
                        token.start_position,
                        "Unclosed flow collection at end of stream",
                    ));
                }
                // YAML 1.2 §8.1.3.1: an implicit mapping key must be
                // followed by `:`. A bare scalar at a mapping position
                // with no \`:\` (and no explicit `?` marker) is invalid
                // (yaml-test-suite 7MNF).
                if matches!(self.state, ParserState::BlockMappingKey)
                    && !self.explicit_key_pending
                    && innermost_mapping_has_odd_children(&self.events)
                {
                    return Err(Error::parse(
                        token.start_position,
                        "Mapping key not followed by `:`",
                    ));
                }
                // YAML 1.2: an explicit `---` with NO body needs an
                // implicit empty scalar as the doc's content (yaml-test-
                // suite MUS6/02). We detect that case by checking the
                // last emitted event — if it's still `DocumentStart`,
                // nothing has been pushed to the body yet.
                if matches!(
                    self.events.last().map(|e| &e.event_type),
                    Some(EventType::DocumentStart { .. })
                ) {
                    self.events.push(Event::scalar(
                        token.start_position,
                        None,
                        None,
                        String::new(),
                        true,
                        false,
                        ScalarStyle::Plain,
                    ));
                }
                // Close any open document. A document is "open" in every
                // state except: not-yet-started (StreamStart /
                // ImplicitDocumentStart), or already closed (DocumentEnd /
                // StreamEnd). If still open, also flush any unclosed
                // block collections first.
                if !matches!(
                    self.state,
                    ParserState::StreamStart
                        | ParserState::ImplicitDocumentStart
                        | ParserState::DocumentEnd
                        | ParserState::StreamEnd
                ) {
                    close_open_collections(&mut self.events, token.start_position);
                    self.events
                        .push(Event::document_end(token.start_position, true));
                }
                self.events.push(Event::stream_end(token.start_position));
                self.state = ParserState::StreamEnd;
            }

            TokenType::YamlDirective(major, minor) => {
                // YAML 1.2 §6.8: directives may appear only before the
                // first document or after an explicit `...` document end.
                self.check_directive_context(token.start_position, "%YAML")?;
                // §6.8.1: a document may have at most one `%YAML` directive.
                if self.yaml_version.is_some() {
                    return Err(Error::parse(
                        token.start_position,
                        "Multiple %YAML directives in the same document",
                    ));
                }
                self.yaml_version = Some((*major, *minor));
            }

            TokenType::TagDirective(handle, prefix) => {
                self.check_directive_context(token.start_position, "%TAG")?;
                self.tag_directives.push((handle.clone(), prefix.clone()));
                self.tag_resolver
                    .add_directive(handle.clone(), prefix.clone());
            }

            TokenType::DocumentStart => {
                // If the most-recent event is still `DocumentStart`, the
                // previous document had no body — emit an implicit empty
                // scalar before closing it (yaml-test-suite 6XDY).
                if matches!(
                    self.events.last().map(|e| &e.event_type),
                    Some(EventType::DocumentStart { .. })
                ) {
                    self.events.push(Event::scalar(
                        token.start_position,
                        None,
                        None,
                        String::new(),
                        true,
                        false,
                        ScalarStyle::Plain,
                    ));
                    self.events
                        .push(Event::document_end(token.start_position, true));
                } else if has_open_document(&self.events) {
                    // The previous document is still open — its outer
                    // collection(s) and the document itself need closing
                    // before the new `---` (yaml-test-suite 35KP).
                    close_open_collections(&mut self.events, token.start_position);
                    self.events
                        .push(Event::document_end(token.start_position, true));
                }

                // Create document start with directives
                self.events.push(Event::document_start(
                    token.start_position,
                    self.yaml_version.take(),
                    self.tag_directives.clone(),
                    false,
                ));

                // Clear tag directives after using them (YAML version persists across documents)
                // But keep them in the tag resolver for this document
                self.tag_directives.clear();

                self.state = ParserState::DocumentStart;
            }

            TokenType::DocumentEnd => {
                // Same empty-doc fixup as in DocumentStart/StreamEnd:
                // `---\n...` needs an implicit empty scalar.
                if matches!(
                    self.events.last().map(|e| &e.event_type),
                    Some(EventType::DocumentStart { .. })
                ) {
                    self.events.push(Event::scalar(
                        token.start_position,
                        None,
                        None,
                        String::new(),
                        true,
                        false,
                        ScalarStyle::Plain,
                    ));
                } else {
                    // Flush any still-open block collections so the
                    // event stream is balanced before -DOC.
                    close_open_collections(&mut self.events, token.start_position);
                }
                self.events
                    .push(Event::document_end(token.start_position, false));
                // YAML 1.2: after `...`, the stream may continue with
                // either another `---`, more directives, or implicit
                // document content.
                self.state = ParserState::ImplicitDocumentStart;
            }

            TokenType::BlockSequenceStart => {
                // §3.2.1.1: reject a second root-level node
                // (yaml-test-suite BD7L: `- a\n- b\ninvalid: x`).
                if matches!(self.state, ParserState::DocumentContent)
                    && second_root_node_present(&self.events)
                {
                    return Err(Error::parse(
                        token.start_position,
                        "Document already contains a root node",
                    ));
                }
                if matches!(self.state, ParserState::ImplicitDocumentStart) {
                    let event = self.create_implicit_document_start(token.start_position);
                    self.events.push(event);
                }

                // If we're starting a sequence within a mapping context, push the current state
                if matches!(
                    self.state,
                    ParserState::BlockMappingValue | ParserState::BlockMappingKey
                ) {
                    self.state_stack.push(self.state);
                }

                self.events.push(Event::sequence_start(
                    token.start_position,
                    self.pending_anchor.take(),
                    self.pending_tag.take(),
                    false,
                ));
                self.state = ParserState::BlockSequence;
            }

            TokenType::BlockMappingStart => {
                // §3.2.1.1: reject a second root-level node
                // (yaml-test-suite BD7L variants).
                if matches!(self.state, ParserState::DocumentContent)
                    && second_root_node_present(&self.events)
                {
                    return Err(Error::parse(
                        token.start_position,
                        "Document already contains a root node",
                    ));
                }
                // Determine whether to create a new mapping or continue existing one
                // This token is generated when we encounter a key at the start of a line with nested content
                // It doesn't always mean we need to create a new mapping - sometimes we're just continuing

                let should_create_new_mapping = match self.state {
                    ParserState::ImplicitDocumentStart => {
                        // At document start, we need a new mapping
                        true
                    }
                    ParserState::DocumentStart => {
                        // After explicit document start (---), we need a new mapping
                        true
                    }
                    ParserState::DocumentContent => {
                        // This is a tricky case - we could be:
                        // 1. Starting a new root mapping
                        // 2. Continuing an existing root mapping
                        // The key is to check if we have an unclosed root mapping

                        // Count mapping depth from the end
                        let mut mapping_depth = 0;
                        let mut has_unclosed_mapping = false;

                        for event in self.events.iter().rev() {
                            match &event.event_type {
                                EventType::MappingEnd => mapping_depth += 1,
                                EventType::MappingStart { .. } => {
                                    if mapping_depth == 0 {
                                        has_unclosed_mapping = true;
                                        break;
                                    }
                                    mapping_depth -= 1;
                                }
                                EventType::DocumentStart { .. } => break,
                                _ => {}
                            }
                        }

                        // Don't create a new mapping if we have an unclosed one
                        !has_unclosed_mapping
                    }
                    ParserState::BlockMappingValue => {
                        // If we're expecting a value and see BlockMappingStart, it's a nested mapping
                        true
                    }
                    ParserState::BlockMappingKey => {
                        // We're already in a mapping key context
                        // BlockMappingStart here means we're continuing the mapping unless:
                        // - After a Key token (complex key)
                        // - After a Value token (nested mapping as value)
                        matches!(
                            &self.last_token_type,
                            Some(TokenType::Key | TokenType::Value)
                        )
                    }
                    ParserState::BlockSequence => {
                        // In a sequence context, BlockMappingStart means we're starting
                        // a nested mapping as a sequence item
                        true
                    }
                    _ => {
                        // For other states, check last token
                        matches!(
                            &self.last_token_type,
                            Some(TokenType::Key | TokenType::Value)
                        )
                    }
                };

                if should_create_new_mapping {
                    // Create a new nested mapping
                    if matches!(self.state, ParserState::ImplicitDocumentStart) {
                        let event = self.create_implicit_document_start(token.start_position);
                        self.events.push(event);
                    }

                    // If we're in a mapping value or sequence context, push state to stack
                    if matches!(
                        self.state,
                        ParserState::BlockMappingValue | ParserState::BlockSequence
                    ) {
                        self.state_stack.push(self.state);
                    }

                    // If the BlockMappingStart wraps an implicit key
                    // at the document root and the next token is the
                    // key scalar, any pending anchor/tag belongs to
                    // that key — not to the surrounding mapping
                    // (yaml-test-suite ZH7C, E76Z, 74H7). For mappings
                    // nested in a value or sequence position the
                    // anchor was placed in the value slot and DOES
                    // attach to the mapping itself.
                    let in_value_position = matches!(
                        self.state,
                        ParserState::BlockMappingValue | ParserState::BlockSequence
                    );
                    let next_is_scalar = matches!(
                        self.scanner.peek_token(),
                        Ok(Some(t)) if matches!(
                            t.token_type,
                            TokenType::Scalar(..) | TokenType::Anchor(_) | TokenType::Tag(_)
                        )
                    );
                    let (anchor, tag) = if !in_value_position && next_is_scalar {
                        (None, None)
                    } else {
                        (self.pending_anchor.take(), self.pending_tag.take())
                    };
                    self.events.push(Event::mapping_start(
                        token.start_position,
                        anchor,
                        tag,
                        false,
                    ));
                    self.state = ParserState::BlockMappingKey;
                } else {
                    // Continue existing mapping
                    // Ensure we're in the right state to handle the next key-value pair
                    match self.state {
                        ParserState::DocumentContent => {
                            // We should be continuing a mapping, so transition to BlockMappingKey
                            self.state = ParserState::BlockMappingKey;
                        }
                        ParserState::BlockMappingValue => {
                            // We just processed a value, now ready for next key
                            self.state = ParserState::BlockMappingKey;
                        }
                        ParserState::BlockMappingKey => {
                            // Already ready for next key, no state change needed
                        }
                        _ => {
                            // For other states, check if we can restore from state stack
                            if let Some(prev_state) = self.state_stack.last() {
                                if matches!(prev_state, ParserState::BlockMappingValue) {
                                    if let Some(mapping_state) = self.state_stack.pop() {
                                        self.state = mapping_state;
                                        self.handle_node_completion();
                                    }
                                }
                            }
                        }
                    }
                }
            }

            TokenType::FlowSequenceStart => {
                if matches!(self.state, ParserState::ImplicitDocumentStart) {
                    self.events.push(Event::document_start(
                        token.start_position,
                        None,
                        vec![],
                        true,
                    ));
                }

                // Save block context state so we can restore it after the flow collection
                if matches!(
                    self.state,
                    ParserState::BlockMappingValue
                        | ParserState::BlockMappingKey
                        | ParserState::BlockSequence
                        | ParserState::FlowSequence
                        | ParserState::FlowMapping
                ) {
                    self.state_stack.push(self.state);
                }

                self.events.push(Event::sequence_start(
                    token.start_position,
                    self.pending_anchor.take(),
                    self.pending_tag.take(),
                    true,
                ));
                self.state = ParserState::FlowSequence;
            }

            TokenType::FlowMappingStart => {
                if matches!(self.state, ParserState::ImplicitDocumentStart) {
                    self.events.push(Event::document_start(
                        token.start_position,
                        None,
                        vec![],
                        true,
                    ));
                }

                // Save block context state so we can restore it after the flow collection
                if matches!(
                    self.state,
                    ParserState::BlockMappingValue
                        | ParserState::BlockMappingKey
                        | ParserState::BlockSequence
                        | ParserState::FlowSequence
                        | ParserState::FlowMapping
                ) {
                    self.state_stack.push(self.state);
                }

                self.events.push(Event::mapping_start(
                    token.start_position,
                    self.pending_anchor.take(),
                    self.pending_tag.take(),
                    true,
                ));
                self.state = ParserState::FlowMapping;
            }

            TokenType::FlowSequenceEnd => {
                // §7.5: close an open implicit single-pair flow mapping
                // before the outer flow sequence ends.
                if self.implicit_flow_pair_depth > 0
                    && matches!(
                        self.state,
                        ParserState::FlowMapping
                            | ParserState::FlowMappingKey
                            | ParserState::FlowMappingValue
                    )
                    && matches!(self.state_stack.last(), Some(ParserState::FlowSequence))
                {
                    if innermost_mapping_has_odd_children(&self.events) {
                        self.events.push(Event::scalar(
                            token.start_position,
                            None,
                            None,
                            String::new(),
                            true,
                            false,
                            ScalarStyle::Plain,
                        ));
                    }
                    self.events.push(Event::mapping_end(token.start_position));
                    self.state = self.state_stack.pop().unwrap();
                    self.implicit_flow_pair_depth -= 1;
                }
                self.events.push(Event::sequence_end(token.start_position));

                // Restore the previous state from the stack if available
                if let Some(prev_state) = self.state_stack.pop() {
                    self.state = prev_state;
                } else {
                    self.state = ParserState::DocumentContent;
                }

                // Handle state transitions for mapping key/value processing
                self.handle_node_completion();
            }

            TokenType::FlowMappingEnd => {
                // Spec §7.5: implicit empty value for a flow-mapping
                // entry that has only a key, e.g. `{ key }` or
                // `{ key, a: b }` (yaml-test-suite 8KB6).
                if innermost_mapping_has_odd_children(&self.events) {
                    self.events.push(Event::scalar(
                        token.start_position,
                        None,
                        None,
                        String::new(),
                        true,
                        false,
                        ScalarStyle::Plain,
                    ));
                }
                self.events.push(Event::mapping_end(token.start_position));

                // Restore the previous state from the stack if available
                if let Some(prev_state) = self.state_stack.pop() {
                    self.state = prev_state;
                } else {
                    self.state = ParserState::DocumentContent;
                }

                // Handle state transitions for mapping key/value processing
                self.handle_node_completion();
            }

            TokenType::BlockEnd => {
                // Determine what we're ending based on current state
                match self.state {
                    ParserState::BlockSequence => {
                        self.events.push(Event::sequence_end(token.start_position));
                        // Pop previous state from stack if available
                        if let Some(prev_state) = self.state_stack.pop() {
                            self.state = prev_state;
                            // Handle state transitions for mapping key/value processing
                            self.handle_node_completion();
                        } else {
                            self.state = ParserState::DocumentContent;
                        }
                    }
                    ParserState::BlockMapping
                    | ParserState::BlockMappingKey
                    | ParserState::BlockMappingValue => {
                        // §6.9.1: if the innermost mapping has odd
                        // children (last key has no value), synth an
                        // implicit empty value before closing
                        // (yaml-test-suite 7W2P). If the unmatched key
                        // came from a bare scalar with no `:`
                        // (yaml-test-suite 7MNF), error instead.
                        if innermost_mapping_has_odd_children(&self.events) {
                            if matches!(self.state, ParserState::BlockMappingKey)
                                && !self.explicit_key_pending
                            {
                                return Err(Error::parse(
                                    token.start_position,
                                    "Mapping key not followed by `:`",
                                ));
                            }
                            self.events.push(Event::scalar(
                                token.start_position,
                                None,
                                None,
                                String::new(),
                                true,
                                false,
                                ScalarStyle::Plain,
                            ));
                        }
                        self.events.push(Event::mapping_end(token.start_position));
                        // Pop previous state from stack if available
                        if let Some(prev_state) = self.state_stack.pop() {
                            self.state = prev_state;
                            // If we popped back to a mapping value state, complete it
                            if matches!(self.state, ParserState::BlockMappingValue) {
                                self.handle_node_completion();
                            }
                        } else {
                            // No state on stack - check if we're still in a root mapping
                            // Count the mapping depth including the one we just closed
                            let mut mapping_depth = 0;

                            for event in self.events.iter().rev() {
                                match &event.event_type {
                                    EventType::MappingEnd => {
                                        mapping_depth += 1;
                                    }
                                    EventType::MappingStart { .. } => {
                                        if mapping_depth > 0 {
                                            mapping_depth -= 1;
                                        } else {
                                            // Found an unclosed mapping - we're still in the root mapping
                                            self.state = ParserState::BlockMappingKey;
                                            return Ok(());
                                        }
                                    }
                                    EventType::DocumentStart { .. } => break,
                                    _ => {}
                                }
                            }

                            // All mappings are closed
                            self.state = ParserState::DocumentContent;
                        }
                    }
                    _ => {}
                }
            }

            TokenType::Scalar(value, quote_style) => {
                if matches!(self.state, ParserState::ImplicitDocumentStart) {
                    self.events.push(Event::document_start(
                        token.start_position,
                        None,
                        vec![],
                        true,
                    ));
                    self.state = ParserState::DocumentContent;
                }

                // §3.2.1.1: a document has exactly one root node.
                if matches!(self.state, ParserState::DocumentContent)
                    && second_root_node_present(&self.events)
                {
                    return Err(Error::parse(
                        token.start_position,
                        "Document already contains a root node",
                    ));
                }

                // Check if we're in a sequence and the next token is Value (indicating a mapping key)
                if matches!(self.state, ParserState::BlockSequence) {
                    if let Ok(Some(next_token)) = self.scanner.peek_token() {
                        if matches!(next_token.token_type, TokenType::Value) {
                            // This scalar is a mapping key within a sequence item
                            // Push current state to stack and start a new mapping
                            self.state_stack.push(self.state);
                            self.events.push(Event::mapping_start(
                                token.start_position,
                                self.pending_anchor.take(),
                                self.pending_tag.take(),
                                false,
                            ));
                            self.state = ParserState::BlockMappingKey;
                        }
                    }
                }

                // §7.5: a flow-sequence entry that is itself `key: value`
                // is an implicit single-pair flow mapping
                // (yaml-test-suite QF4Y, L9U5, 87E4, 8UDB, 9MMW, LX3P).
                if matches!(self.state, ParserState::FlowSequence) {
                    if let Ok(Some(next_token)) = self.scanner.peek_token() {
                        if matches!(next_token.token_type, TokenType::Value) {
                            self.state_stack.push(self.state);
                            self.events.push(Event::mapping_start(
                                token.start_position,
                                self.pending_anchor.take(),
                                self.pending_tag.take(),
                                true,
                            ));
                            self.state = ParserState::FlowMappingKey;
                            self.implicit_flow_pair_depth += 1;
                        }
                    }
                }

                // YAML 1.2: if we're in BlockMappingValue and the next
                // token is `:` (Value), the current scalar is actually a
                // NEW KEY — the previous key's value is implicit empty
                // (yaml-test-suite 6KGN: `a: &anchor\nb: *anchor`).
                // Emit the empty value first (consuming any pending
                // anchor/tag — those were intended for the missing
                // value), then transition back to BlockMappingKey.
                //
                // BUT skip the heuristic when:
                //   * the most recent event was an implicit empty scalar
                //     (we just synthesised an empty key for a leading-`:`
                //     mapping, yaml-test-suite 2JQS), or
                //   * the current scalar is on the SAME line as the
                //     previous `:` Value token — that puts the scalar
                //     in the value slot of the current key
                //     (yaml-test-suite 6M2F: `? &a a\n: &b b\n: *a`).
                if matches!(self.state, ParserState::BlockMappingValue) {
                    let last_was_implicit_empty =
                        matches!(self.events.last(), Some(ev) if matches!(
                            &ev.event_type,
                            EventType::Scalar { value, plain_implicit: true, style: ScalarStyle::Plain, .. }
                                if value.is_empty()
                        ));
                    let same_line_as_value = self
                        .last_value_token_line
                        .map_or(false, |line| line == token.start_position.line);
                    if !last_was_implicit_empty && !same_line_as_value {
                        if let Ok(Some(next_token)) = self.scanner.peek_token() {
                            if matches!(next_token.token_type, TokenType::Value) {
                                self.events.push(Event::scalar(
                                    token.start_position,
                                    self.pending_anchor.take(),
                                    self.pending_tag.take(),
                                    String::new(),
                                    true,
                                    false,
                                    ScalarStyle::Plain,
                                ));
                                self.state = ParserState::BlockMappingKey;
                            }
                        }
                    }
                }

                // YAML 1.2 §6.9.1: if we're back at a key position but the
                // previous key still owes a value (odd children in the
                // active mapping), synthesise the implicit empty scalar
                // now — this scalar then becomes the next key
                // (yaml-test-suite 7W2P: `? a\n? b\nc:`).
                if matches!(self.state, ParserState::BlockMappingKey)
                    && innermost_mapping_has_odd_children(&self.events)
                {
                    self.events.push(Event::scalar(
                        token.start_position,
                        None,
                        None,
                        String::new(),
                        true,
                        false,
                        ScalarStyle::Plain,
                    ));
                }

                // Convert QuoteStyle to ScalarStyle
                let style = match quote_style {
                    crate::scanner::QuoteStyle::Plain => ScalarStyle::Plain,
                    crate::scanner::QuoteStyle::Single => ScalarStyle::SingleQuoted,
                    crate::scanner::QuoteStyle::Double => ScalarStyle::DoubleQuoted,
                };

                self.events.push(Event::scalar(
                    token.start_position,
                    self.pending_anchor.take(), // Use pending anchor
                    self.pending_tag.take(),    // Use pending tag
                    value.clone(),
                    style == ScalarStyle::Plain,
                    style != ScalarStyle::Plain,
                    style,
                ));

                // Handle state transitions for mapping key/value processing
                self.handle_node_completion();
            }

            TokenType::BlockScalarLiteral(value) => {
                if matches!(self.state, ParserState::ImplicitDocumentStart) {
                    self.events.push(Event::document_start(
                        token.start_position,
                        None,
                        vec![],
                        true,
                    ));
                    self.state = ParserState::DocumentContent;
                }

                self.events.push(Event::scalar(
                    token.start_position,
                    self.pending_anchor.take(), // Use pending anchor
                    self.pending_tag.take(),    // Use pending tag
                    value.clone(),
                    false, // Not plain
                    true,  // Quoted style
                    ScalarStyle::Literal,
                ));

                // Handle state transitions for mapping key/value processing
                self.handle_node_completion();
            }

            TokenType::BlockScalarFolded(value) => {
                if matches!(self.state, ParserState::ImplicitDocumentStart) {
                    self.events.push(Event::document_start(
                        token.start_position,
                        None,
                        vec![],
                        true,
                    ));
                    self.state = ParserState::DocumentContent;
                }

                self.events.push(Event::scalar(
                    token.start_position,
                    self.pending_anchor.take(), // Use pending anchor
                    self.pending_tag.take(),    // Use pending tag
                    value.clone(),
                    false, // Not plain
                    true,  // Quoted style
                    ScalarStyle::Folded,
                ));

                // Handle state transitions for mapping key/value processing
                self.handle_node_completion();
            }

            TokenType::BlockEntry => {
                // Block sequence entry - this indicates a new item in a sequence
                // We need to ensure proper state management for nested structures
                match self.state {
                    ParserState::BlockSequence => {
                        // We're already in a sequence, this is a new item
                        // No event needed, but we should be ready for the next item
                    }
                    ParserState::BlockMapping | ParserState::BlockMappingValue => {
                        // If we encounter a BlockEntry while in a mapping,
                        // we need to close the mapping and continue the sequence
                        self.events.push(Event::mapping_end(token.start_position));
                        self.state = ParserState::BlockSequence;
                    }
                    _ => {
                        // BlockEntry in other contexts might indicate we need to start a sequence
                        // This handles implicit sequence starts
                        if matches!(self.state, ParserState::ImplicitDocumentStart) {
                            self.events.push(Event::document_start(
                                token.start_position,
                                None,
                                vec![],
                                true,
                            ));
                        }

                        // Start an implicit sequence if we're not already in one
                        self.events.push(Event::sequence_start(
                            token.start_position,
                            self.pending_anchor.take(),
                            self.pending_tag.take(),
                            false,
                        ));
                        self.state = ParserState::BlockSequence;
                    }
                }
            }

            TokenType::Value => {
                self.last_value_token_line = Some(token.start_position.line);
                self.explicit_key_pending = false;
                // YAML 1.2 §6.9.1: a `:` with no preceding key implies an
                // empty key. Handle the four states where this can arise:
                //   * ImplicitDocumentStart — open `+DOC`, `+MAP`, empty key.
                //   * DocumentContent — open `+MAP`, empty key.
                //   * BlockMappingKey with EVEN children — empty key for
                //     the next entry (no scalar preceded the `:`).
                //   * Normal cases (`BlockMappingKey` with odd children,
                //     `FlowMappingKey`) — just transition state.
                match self.state {
                    ParserState::ImplicitDocumentStart => {
                        let event =
                            self.create_implicit_document_start(token.start_position);
                        self.events.push(event);
                        // The mapping itself has no anchor/tag here —
                        // those (if any) belong to the (empty) key.
                        self.events.push(Event::mapping_start(
                            token.start_position,
                            None,
                            None,
                            false,
                        ));
                        self.events.push(Event::scalar(
                            token.start_position,
                            self.pending_anchor.take(),
                            self.pending_tag.take(),
                            String::new(),
                            true,
                            false,
                            ScalarStyle::Plain,
                        ));
                        self.state = ParserState::BlockMappingValue;
                    }
                    ParserState::DocumentContent | ParserState::DocumentStart => {
                        self.events.push(Event::mapping_start(
                            token.start_position,
                            None,
                            None,
                            false,
                        ));
                        self.events.push(Event::scalar(
                            token.start_position,
                            self.pending_anchor.take(),
                            self.pending_tag.take(),
                            String::new(),
                            true,
                            false,
                            ScalarStyle::Plain,
                        ));
                        self.state = ParserState::BlockMappingValue;
                    }
                    ParserState::BlockMappingKey => {
                        if !innermost_mapping_has_odd_children(&self.events) {
                            // Missing key — synthesise empty scalar
                            // first. Pending anchor/tag belongs to that
                            // empty key (yaml-test-suite PW8X).
                            self.events.push(Event::scalar(
                                token.start_position,
                                self.pending_anchor.take(),
                                self.pending_tag.take(),
                                String::new(),
                                true,
                                false,
                                ScalarStyle::Plain,
                            ));
                        }
                        self.state = ParserState::BlockMappingValue;
                    }
                    ParserState::FlowMappingKey => {
                        self.state = ParserState::FlowMappingValue;
                    }
                    _ => {}
                }
            }

            TokenType::FlowEntry => {
                // YAML 1.2 §7.4: a `,` must follow an entry. Leading
                // `,` (e.g. `[ , a, b ]`) and double `,, ` are invalid
                // (yaml-test-suite 9MAG).
                let no_prior_entry = matches!(
                    self.events.last().map(|e| &e.event_type),
                    Some(EventType::SequenceStart { flow_style: true, .. })
                        | Some(EventType::MappingStart { flow_style: true, .. })
                );
                if no_prior_entry {
                    return Err(Error::parse(
                        token.start_position,
                        "Flow entry separator `,` with no preceding entry",
                    ));
                }
                // §7.5: inside a flow mapping, a comma terminates the
                // current entry. If the entry had only a key (no `:`),
                // synth an implicit empty value (yaml-test-suite 8KB6,
                // 9BXH).
                if matches!(
                    self.state,
                    ParserState::FlowMapping | ParserState::FlowMappingKey
                ) && innermost_mapping_has_odd_children(&self.events)
                {
                    self.events.push(Event::scalar(
                        token.start_position,
                        None,
                        None,
                        String::new(),
                        true,
                        false,
                        ScalarStyle::Plain,
                    ));
                    self.state = ParserState::FlowMapping;
                }

                // §7.5: same close-on-comma logic for implicit
                // single-pair mappings.
                if self.implicit_flow_pair_depth > 0
                    && matches!(
                        self.state,
                        ParserState::FlowMapping
                            | ParserState::FlowMappingKey
                            | ParserState::FlowMappingValue
                    )
                    && matches!(self.state_stack.last(), Some(ParserState::FlowSequence))
                {
                    if innermost_mapping_has_odd_children(&self.events) {
                        self.events.push(Event::scalar(
                            token.start_position,
                            None,
                            None,
                            String::new(),
                            true,
                            false,
                            ScalarStyle::Plain,
                        ));
                    }
                    self.events.push(Event::mapping_end(token.start_position));
                    self.state = self.state_stack.pop().unwrap();
                    self.implicit_flow_pair_depth -= 1;
                }
            }

            TokenType::Anchor(name) => {
                // YAML 1.2 §6.9.2: a node may have at most one anchor.
                // A second anchor before the node is consumed is invalid
                // (yaml-test-suite 4JVG).
                if self.pending_anchor.is_some() {
                    return Err(Error::parse(
                        token.start_position,
                        "Node may not have more than one anchor",
                    ));
                }
                // Record the anchor name so subsequent aliases can be
                // validated against it (YAML 1.2 §6.9.2 forbids forward
                // references).
                self.defined_anchors.insert(name.clone());
                self.pending_anchor = Some(name.clone());
            }

            TokenType::Alias(name) => {
                // YAML 1.2 §6.9.2: alias must reference a previously
                // defined anchor — forward references are invalid.
                if !self.defined_anchors.contains(name.as_str()) {
                    return Err(Error::parse(
                        token.start_position,
                        format!("Alias `*{name}` references an undefined anchor"),
                    ));
                }
                // §6.9.2: an alias is a reference, not an independent
                // node — it cannot carry an anchor or tag of its own
                // (yaml-test-suite SR86, SU74).
                if self.pending_anchor.is_some() || self.pending_tag.is_some() {
                    return Err(Error::parse(
                        token.start_position,
                        "Alias may not have an anchor or tag",
                    ));
                }
                if matches!(self.state, ParserState::ImplicitDocumentStart) {
                    self.events.push(Event::document_start(
                        token.start_position,
                        None,
                        vec![],
                        true,
                    ));
                    self.state = ParserState::DocumentContent;
                }

                // Generate alias event
                self.events
                    .push(Event::alias(token.start_position, name.clone()));

                // Handle state transitions for mapping key/value processing
                self.handle_node_completion();
            }

            TokenType::Tag(tag) => {
                // YAML 1.2 §6.9.1 allows at most one tag per node, but
                // (like the double-anchor check) detecting that at this
                // layer produces false positives — a tag preceding an
                // implicit empty node in a sequence is followed by the
                // tag of the next sibling node, and the same `pending_tag`
                // field is reused. Until the parser tracks per-node tag
                // scopes, accept the overwrite silently (yaml-test-suite
                // FH7J relies on this).
                // Resolve and normalize the tag before storing.
                match self.tag_resolver.resolve(&tag) {
                    Ok(resolved_tag) => {
                        self.pending_tag = Some(resolved_tag.uri);
                    }
                    Err(_) => {
                        self.pending_tag = Some(tag.clone());
                    }
                }
            }

            // TODO: Implement these when we add support for advanced features
            TokenType::Comment(_) => {
                // Not implemented in basic version
            }

            // Complex key marker
            TokenType::Key => {
                self.explicit_key_pending = true;
                match self.state {
                    ParserState::ImplicitDocumentStart => {
                        // Start implicit document and mapping
                        let event = self.create_implicit_document_start(token.start_position);
                        self.events.push(event);
                        self.events.push(Event::mapping_start(
                            token.start_position,
                            self.pending_anchor.take(),
                            self.pending_tag.take(),
                            false,
                        ));
                        self.state = ParserState::BlockMappingKey;
                    }
                    ParserState::DocumentStart => {
                        // Explicit document start (`---`) followed by a
                        // complex-key marker — open the document body as
                        // an implicit block mapping (yaml-test-suite 2XXW).
                        self.events.push(Event::mapping_start(
                            token.start_position,
                            self.pending_anchor.take(),
                            self.pending_tag.take(),
                            false,
                        ));
                        self.state = ParserState::BlockMappingKey;
                    }
                    ParserState::DocumentContent => {
                        // Check if we just finished a mapping - if so, continue it instead of starting new one
                        // This happens when the previous mapping key-value pair was processed but no BlockEnd was generated
                        if !self.events.is_empty() {
                            if let Some(last_event) = self.events.last() {
                                // If the last event was a scalar and we have a MappingStart before it,
                                // we're probably continuing an existing mapping
                                if matches!(last_event.event_type, EventType::Scalar { .. }) {
                                    // Look for a recent MappingStart without a corresponding MappingEnd
                                    let mut mapping_depth = 0;
                                    let mut has_unfinished_mapping = false;

                                    for event in self.events.iter().rev() {
                                        match &event.event_type {
                                            EventType::MappingEnd => mapping_depth += 1,
                                            EventType::MappingStart { .. } => {
                                                if mapping_depth == 0 {
                                                    has_unfinished_mapping = true;
                                                    break;
                                                }
                                                mapping_depth -= 1;
                                            }
                                            _ => {}
                                        }
                                    }

                                    if has_unfinished_mapping {
                                        // Continue the existing mapping instead of starting a new one
                                        self.state = ParserState::BlockMappingKey;
                                        return Ok(());
                                    }
                                }
                            }
                        }

                        // Start new mapping
                        self.events.push(Event::mapping_start(
                            token.start_position,
                            self.pending_anchor.take(),
                            self.pending_tag.take(),
                            false,
                        ));
                        self.state = ParserState::BlockMappingKey;
                    }
                    ParserState::BlockMapping | ParserState::FlowMapping => {
                        // Already in a mapping, now we have a complex key
                        self.state = if matches!(self.state, ParserState::BlockMapping) {
                            ParserState::BlockMappingKey
                        } else {
                            ParserState::FlowMappingKey
                        };
                    }
                    ParserState::BlockMappingKey | ParserState::FlowMappingKey => {
                        // A new `?` while we still owe a value for the
                        // previous key — synthesise an implicit empty
                        // scalar so the mapping stays balanced
                        // (yaml-test-suite 7W2P).
                        if innermost_mapping_has_odd_children(&self.events) {
                            self.events.push(Event::scalar(
                                token.start_position,
                                None,
                                None,
                                String::new(),
                                true,
                                false,
                                ScalarStyle::Plain,
                            ));
                        }
                    }
                    _ => {
                        let context =
                            ErrorContext::from_input(self.scanner.input(), &self.position, 2)
                                .with_suggestion(
                                    "Complex keys must be used in mapping contexts".to_string(),
                                );
                        return Err(Error::parse_with_context(
                            self.position,
                            "Complex key marker (?) in invalid context",
                            context,
                        ));
                    }
                }
            }
        }

        // Update the last token type for next iteration
        self.last_token_type = token_type_for_tracking;

        Ok(())
    }

    /// Handle completion of a node (scalar or collection) and manage mapping state transitions
    #[allow(clippy::missing_const_for_fn)]
    fn handle_node_completion(&mut self) {
        match self.state {
            ParserState::BlockMappingKey => {
                // After processing a key, we stay in BlockMappingKey state
                // The Value token (:) will transition us to BlockMappingValue
                // No state change needed here
            }
            ParserState::FlowMappingKey => {
                // After processing a key in flow mapping, we stay in FlowMappingKey state
                // The Value token (:) will transition us to FlowMappingValue
                // No state change needed here
            }
            ParserState::BlockMappingValue => {
                // After processing a value, we go back to waiting for the next key
                self.state = ParserState::BlockMappingKey;
            }
            ParserState::FlowMappingValue => {
                // After processing a value in flow mapping, we go back to waiting for the next key
                self.state = ParserState::FlowMapping;
            }
            _ => {
                // No state change needed for other states
            }
        }
    }
}

impl Default for BasicParser {
    fn default() -> Self {
        Self::new(String::new())
    }
}

impl Parser for BasicParser {
    fn check_event(&self) -> bool {
        // For streaming: check if we have cached events or can generate more
        self.event_index < self.events.len() || self.scanner.check_token()
    }

    fn peek_event(&self) -> Result<Option<&Event>> {
        // Peek at cached events only (don't generate new ones)
        Ok(self.events.get(self.event_index))
    }

    fn get_event(&mut self) -> Result<Option<Event>> {
        // Generate next events until we have one available
        // Some tokens (like directives) don't generate events
        while self.event_index >= self.events.len() && self.scanner.check_token() {
            let events_before = self.events.len();
            self.generate_next_event()?;

            // If no event was generated and we still have tokens, continue
            if self.events.len() == events_before && self.scanner.check_token() {
                continue;
            }
            break;
        }

        if self.event_index < self.events.len() {
            let event = self.events[self.event_index].clone();
            self.event_index += 1;
            Ok(Some(event))
        } else {
            Ok(None)
        }
    }

    fn reset(&mut self) {
        self.event_index = 0;
        self.scanner.reset();
        self.state_stack.clear();
        self.position = Position::start();
        self.pending_anchor = None;
        self.pending_tag = None;
        self.last_token_type = None;
    }

    fn position(&self) -> Position {
        self.position
    }
}

impl BasicParser {
    /// Check if there was a scanning error
    #[allow(clippy::missing_const_for_fn)]
    pub fn take_scanning_error(&mut self) -> Option<Error> {
        self.scanning_error.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_parsing() {
        let mut parser = BasicParser::new_eager("42".to_string());

        assert!(parser.check_event());

        // Stream start
        let event = parser.get_event().unwrap().unwrap();
        assert!(matches!(event.event_type, EventType::StreamStart));

        // Document start (implicit)
        let event = parser.get_event().unwrap().unwrap();
        if let EventType::DocumentStart { implicit, .. } = event.event_type {
            assert!(implicit);
        } else {
            panic!("Expected implicit document start");
        }

        // Scalar
        let event = parser.get_event().unwrap().unwrap();
        if let EventType::Scalar { value, .. } = event.event_type {
            assert_eq!(value, "42");
        } else {
            panic!("Expected scalar event");
        }

        // Document end (implicit)
        let event = parser.get_event().unwrap().unwrap();
        if let EventType::DocumentEnd { implicit } = event.event_type {
            assert!(implicit);
        } else {
            panic!("Expected implicit document end");
        }

        // Stream end
        let event = parser.get_event().unwrap().unwrap();
        assert!(matches!(event.event_type, EventType::StreamEnd));
    }

    #[test]
    fn test_flow_sequence_parsing() {
        let mut parser = BasicParser::new_eager("[1, 2, 3]".to_string());

        // Stream start
        parser.get_event().unwrap();

        // Document start (implicit)
        parser.get_event().unwrap();

        // Sequence start
        let event = parser.get_event().unwrap().unwrap();
        if let EventType::SequenceStart { flow_style, .. } = event.event_type {
            assert!(flow_style);
        } else {
            panic!("Expected flow sequence start");
        }

        // First scalar
        let event = parser.get_event().unwrap().unwrap();
        if let EventType::Scalar { value, .. } = event.event_type {
            assert_eq!(value, "1");
        } else {
            panic!("Expected scalar '1'");
        }
    }

    #[test]
    fn test_flow_mapping_parsing() {
        let mut parser = BasicParser::new_eager("{'key': 'value'}".to_string());

        // Stream start
        parser.get_event().unwrap();

        // Document start (implicit)
        parser.get_event().unwrap();

        // Mapping start
        let event = parser.get_event().unwrap().unwrap();
        if let EventType::MappingStart { flow_style, .. } = event.event_type {
            assert!(flow_style);
        } else {
            panic!("Expected flow mapping start");
        }

        // Key scalar
        let event = parser.get_event().unwrap().unwrap();
        if let EventType::Scalar { value, .. } = event.event_type {
            assert_eq!(value, "key");
        } else {
            panic!("Expected scalar 'key'");
        }
    }
}
