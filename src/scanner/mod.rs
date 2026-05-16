//! YAML scanner for tokenization

use crate::{error::ErrorContext, Error, Limits, Position, ResourceTracker, Result};

pub mod indentation;
pub mod scalar_scanner;
pub mod state;
pub mod token_processor;
pub mod tokens;
// pub mod optimizations; // Temporarily disabled
pub use scalar_scanner::ScalarScanner;
pub use tokens::*;
// pub use optimizations::*;

/// Trait for YAML scanners that convert character streams to tokens
pub trait Scanner {
    /// Check if there are more tokens available
    fn check_token(&self) -> bool;

    /// Peek at the next token without consuming it
    fn peek_token(&self) -> Result<Option<&Token>>;

    /// Get the next token, consuming it
    fn get_token(&mut self) -> Result<Option<Token>>;

    /// Reset the scanner state
    fn reset(&mut self);

    /// Get the current position in the input
    fn position(&self) -> Position;

    /// Get the input text for error reporting
    fn input(&self) -> &str;
}

/// A basic scanner implementation for YAML tokenization
#[derive(Debug)]
#[allow(dead_code)]
pub struct BasicScanner {
    input: String,
    position: Position,
    current_char: Option<char>,
    tokens: Vec<Token>,
    token_index: usize,
    done: bool,
    indent_stack: Vec<usize>,
    current_indent: usize,
    allow_simple_key: bool,
    simple_key_allowed: bool,
    flow_level: usize,
    preserve_comments: bool,
    // Indentation style detection
    detected_indent_style: Option<crate::value::IndentStyle>,
    indent_samples: Vec<(usize, bool)>, // (size, is_tabs)
    previous_indent_level: usize,       // Track the previous indentation for style detection
    // Performance optimizations
    buffer: String,                   // Reusable string buffer for token values
    char_cache: Vec<char>,            // Cached characters for faster access
    char_indices: Vec<(usize, char)>, // Cached character indices for O(1) lookups
    current_char_index: usize,        // Current index in char_cache
    profiler: Option<crate::profiling::YamlProfiler>, // Optional profiling
    // Error tracking
    scanning_error: Option<Error>, // Store scanning errors for later retrieval
    // Resource tracking
    limits: Limits,
    resource_tracker: ResourceTracker,
    // Track inline nested sequences that need closing
    inline_sequence_depth: usize,
    // Track compact-notation sequences (where `-` is at the same indent as
    // the parent mapping keys). These are NOT on indent_stack, so we need
    // separate tracking to know when to emit BlockEnd for them.
    compact_sequence_indents: Vec<usize>,
    // Parallel to indent_stack: true when the entry was pushed by a block
    // sequence, false when by a mapping. Lets us distinguish "continuing a
    // regular sequence" from "starting a compact sequence at same indent".
    indent_is_sequence: Vec<bool>,
}

impl BasicScanner {
    /// Create a new scanner from input string
    pub fn new(input: String) -> Self {
        Self::with_limits(input, Limits::default())
    }

    /// Create a new scanner with custom resource limits
    pub fn with_limits(input: String, limits: Limits) -> Self {
        let char_cache: Vec<char> = input.chars().collect();
        let char_indices: Vec<(usize, char)> = input.char_indices().collect();
        let current_char = char_cache.first().copied();

        // Track document size for resource limits
        let mut resource_tracker = ResourceTracker::new();
        if let Err(e) = resource_tracker.add_bytes(&limits, input.len()) {
            // If the input is too large, create scanner with error state
            return Self {
                current_char: None,
                input,
                position: Position::start(),
                tokens: Vec::new(),
                token_index: 0,
                done: true,
                indent_stack: vec![0],
                current_indent: 0,
                allow_simple_key: false,
                simple_key_allowed: false,
                flow_level: 0,
                preserve_comments: false,
                detected_indent_style: None,
                indent_samples: Vec::new(),
                previous_indent_level: 0,
                buffer: String::new(),
                char_cache: Vec::new(),
                char_indices: Vec::new(),
                current_char_index: 0,
                profiler: None,
                scanning_error: Some(e),
                limits,
                resource_tracker,
                inline_sequence_depth: 0,
                compact_sequence_indents: Vec::new(),
                indent_is_sequence: vec![false],
            };
        }

        Self {
            current_char,
            input,
            position: Position::start(),
            tokens: Vec::new(),
            token_index: 0,
            done: false,
            indent_stack: vec![0], // Always start with base indentation
            current_indent: 0,
            allow_simple_key: true,
            simple_key_allowed: true,
            flow_level: 0,
            preserve_comments: false,
            detected_indent_style: None,
            indent_samples: Vec::new(),
            previous_indent_level: 0,
            buffer: String::with_capacity(64), // Pre-allocate buffer
            char_cache,
            char_indices,
            current_char_index: 0,
            profiler: std::env::var("RUST_YAML_PROFILE")
                .ok()
                .map(|_| crate::profiling::YamlProfiler::new()),
            scanning_error: None,
            limits,
            resource_tracker,
            inline_sequence_depth: 0,
            compact_sequence_indents: Vec::new(),
            indent_is_sequence: vec![false],
        }
    }

    /// Create a new scanner with eager token scanning (for compatibility)
    pub fn new_eager(input: String) -> Self {
        Self::new_eager_with_limits(input, Limits::default())
    }

    /// Create a new scanner with eager token scanning and custom limits
    pub fn new_eager_with_limits(input: String, limits: Limits) -> Self {
        let mut scanner = Self::with_limits(input, limits);
        // Store any scanning errors for later retrieval
        if let Err(error) = scanner.scan_all_tokens() {
            scanner.scanning_error = Some(error);
        }
        scanner
    }

    /// Create a new scanner with comment preservation enabled
    pub fn new_with_comments(input: String) -> Self {
        let mut scanner = Self::new(input);
        scanner.preserve_comments = true;
        scanner
    }

    /// Create a new scanner with comments and custom limits
    pub fn new_with_comments_and_limits(input: String, limits: Limits) -> Self {
        let mut scanner = Self::with_limits(input, limits);
        scanner.preserve_comments = true;
        scanner
    }

    /// Create a new scanner with eager scanning and comment preservation
    pub fn new_eager_with_comments(input: String) -> Self {
        let mut scanner = Self::new_with_comments(input);
        scanner.scan_all_tokens().unwrap_or(());
        scanner
    }

    /// Get the detected indentation style from the document
    pub const fn detected_indent_style(&self) -> Option<&crate::value::IndentStyle> {
        self.detected_indent_style.as_ref()
    }

    /// Check if there was a scanning error
    pub const fn has_scanning_error(&self) -> bool {
        self.scanning_error.is_some()
    }

    /// Get the scanning error if any
    #[allow(clippy::missing_const_for_fn)]
    pub fn take_scanning_error(&mut self) -> Option<Error> {
        self.scanning_error.take()
    }

    /// Advance to the next character
    fn advance(&mut self) -> Option<char> {
        if let Some(ch) = self.current_char {
            self.position = self.position.advance(ch);
            self.current_char_index += 1;

            if self.current_char_index < self.char_cache.len() {
                self.current_char = Some(self.char_cache[self.current_char_index]);
            } else {
                self.current_char = None;
            }
        }

        self.current_char
    }

    /// Skip whitespace characters (excluding newlines)
    fn skip_whitespace(&mut self) {
        while let Some(ch) = self.current_char {
            if ch == ' ' || ch == '\t' {
                self.advance();
            } else {
                break;
            }
        }
    }

    /// Handle indentation and produce block tokens if necessary
    fn handle_indentation(&mut self) -> Result<()> {
        // Only handle indentation in block context (flow_level == 0)
        if self.flow_level > 0 {
            return Ok(());
        }

        let line_start_pos = self.position;
        let mut indent = 0;
        let mut has_tabs = false;
        let mut has_spaces = false;
        let _indent_start_pos = self.position;

        // Count indentation and detect style
        while let Some(ch) = self.current_char {
            if ch == ' ' {
                indent += 1;
                has_spaces = true;
                self.advance();
            } else if ch == '\t' {
                indent += 8; // Tab counts as 8 spaces for indentation calculation
                has_tabs = true;
                self.advance();
            } else {
                break;
            }
        }

        // Analyze indentation pattern for style detection
        // Only analyze if there's actual content after the indentation (not just whitespace)
        if indent > 0
            && self.current_char.is_some()
            && !matches!(self.current_char, Some('\n' | '\r'))
        {
            self.analyze_indentation_pattern(indent, has_tabs, has_spaces)?;
        }

        // Perform strict indentation validation if we have established a style
        if let Some(crate::value::IndentStyle::Spaces(width)) = self.detected_indent_style {
            if indent > 0 && indent % width != 0 {
                // Check if this is a valid nested level or inconsistent indentation
                let is_valid_nesting = self.is_valid_indentation_level(indent);
                if !is_valid_nesting {
                    let lower_level = (indent / width) * width;
                    let higher_level = lower_level + width;
                    let suggestion = format!(
                        "Inconsistent indentation detected. Expected multiples of {} spaces. Use {} or {} spaces instead of {}",
                        width, lower_level, higher_level, indent
                    );
                    let context =
                        crate::error::ErrorContext::from_input(&self.input, &self.position, 4)
                            .with_suggestion(suggestion);
                    return Err(Error::indentation_with_context(
                        self.position,
                        lower_level,
                        indent,
                        context,
                    ));
                }
            }
        }

        // Update previous indentation level for future comparisons
        if indent > 0 {
            self.previous_indent_level = indent;
        }

        // Update current indentation level
        self.current_indent = indent;

        // Close compact-notation sequences whose scope ends at this line.
        // A compact sequence (where `-` shares the indent of the parent
        // mapping keys) ends when the next content line at that indent is
        // NOT a block entry (`- `).  We must emit the sequence's BlockEnd
        // BEFORE popping the indent_stack so that the nesting order is
        // correct (sequence closes before its parent mapping).
        let has_content =
            self.current_char.is_some() && !matches!(self.current_char, Some('\n' | '\r' | '#'));
        if has_content {
            let is_block_entry = self.current_char == Some('-')
                && self.peek_char(1).map_or(true, |c| c.is_whitespace());
            while let Some(&seq_indent) = self.compact_sequence_indents.last() {
                if indent < seq_indent || (indent == seq_indent && !is_block_entry) {
                    self.compact_sequence_indents.pop();
                    self.tokens
                        .push(Token::simple(TokenType::BlockEnd, line_start_pos));
                } else {
                    break;
                }
            }
        }

        // Check if we need to emit block end tokens for decreased indentation
        while let Some(&last_indent) = self.indent_stack.last() {
            if indent < last_indent && last_indent > 0 {
                self.indent_stack.pop();
                self.indent_is_sequence.pop();
                self.tokens
                    .push(Token::simple(TokenType::BlockEnd, line_start_pos));
            } else {
                break;
            }
        }

        Ok(())
    }

    /// Analyze indentation pattern to detect the document's indentation style
    fn analyze_indentation_pattern(
        &mut self,
        current_indent: usize,
        has_tabs: bool,
        has_spaces: bool,
    ) -> Result<()> {
        // Prevent mixed indentation (tabs + spaces on same line)
        if has_tabs && has_spaces {
            let context = crate::error::ErrorContext::from_input(&self.input, &self.position, 4)
                .with_suggestion("Use either tabs OR spaces for indentation, not both".to_string());
            return Err(Error::invalid_character_with_context(
                self.position,
                '\t',
                "mixed indentation",
                context,
            ));
        }

        // If we detected tabs, check for mixed indentation across lines
        if has_tabs {
            match self.detected_indent_style {
                None => {
                    // First time detecting indentation style - set to tabs
                    self.detected_indent_style = Some(crate::value::IndentStyle::Tabs);
                }
                Some(crate::value::IndentStyle::Spaces(_)) => {
                    // Previously detected spaces, now seeing tabs - mixed indentation error
                    let context =
                        crate::error::ErrorContext::from_input(&self.input, &self.position, 4)
                            .with_suggestion(
                                "Use consistent indentation style throughout the document"
                                    .to_string(),
                            );
                    return Err(Error::invalid_character_with_context(
                        self.position,
                        '\t',
                        "mixed indentation",
                        context,
                    ));
                }
                Some(crate::value::IndentStyle::Tabs) => {
                    // Already using tabs - this is consistent
                }
            }
            return Ok(());
        }

        // For spaces, check for mixed indentation across lines first
        if has_spaces {
            // Check if we previously detected tabs
            if matches!(
                self.detected_indent_style,
                Some(crate::value::IndentStyle::Tabs)
            ) {
                let context =
                    crate::error::ErrorContext::from_input(&self.input, &self.position, 4)
                        .with_suggestion(
                            "Use consistent indentation style throughout the document".to_string(),
                        );
                return Err(Error::invalid_character_with_context(
                    self.position,
                    ' ',
                    "mixed indentation",
                    context,
                ));
            }

            // Calculate the indentation level difference
            if current_indent > self.previous_indent_level {
                let indent_diff = current_indent - self.previous_indent_level;

                // Store this sample for analysis (but only meaningful differences)
                if indent_diff > 0 && indent_diff <= 8 {
                    // Reasonable indentation range
                    self.indent_samples.push((indent_diff, false));

                    // Try to determine consistent indentation width
                    if self.detected_indent_style.is_none() {
                        self.detect_space_indentation_width();
                    }
                }
            }

            // Validate indentation consistency if we already have a detected style
            self.validate_indentation_consistency(current_indent)?;
        }

        Ok(())
    }

    /// Detect the consistent space indentation width from samples
    fn detect_space_indentation_width(&mut self) {
        if self.indent_samples.is_empty() {
            return; // Need at least 1 sample
        }

        // Find the most common indentation width
        let mut width_counts = std::collections::HashMap::new();

        for &(width, is_tabs) in &self.indent_samples {
            if !is_tabs && width > 0 {
                *width_counts.entry(width).or_insert(0) += 1;
            }
        }

        // Find the most frequent width - be more aggressive and detect early
        if let Some((&most_common_width, &_count)) =
            width_counts.iter().max_by_key(|&(_, count)| count)
        {
            // Set on first consistent sample to enable stricter validation
            self.detected_indent_style = Some(crate::value::IndentStyle::Spaces(most_common_width));
        }
    }

    /// Check if the given indentation level is valid based on current context
    #[allow(clippy::missing_const_for_fn)] // Cannot be const due to self.detected_indent_style access
    fn is_valid_indentation_level(&self, indent: usize) -> bool {
        // For now, allow any indentation that could represent valid nesting
        // In the future, this could be made more strict by checking against
        // the current indent_stack to ensure proper nesting
        if let Some(crate::value::IndentStyle::Spaces(width)) = self.detected_indent_style {
            // Must be a multiple of the detected width
            indent % width == 0
        } else {
            // If no style detected yet, allow any indentation
            true
        }
    }

    /// Validate that current indentation is consistent with detected style
    fn validate_indentation_consistency(&self, current_indent: usize) -> Result<()> {
        if let Some(crate::value::IndentStyle::Spaces(width)) = self.detected_indent_style {
            // Check if current indentation is a multiple of the detected width
            if current_indent > 0 && current_indent % width != 0 {
                let lower_level = (current_indent / width) * width;
                let higher_level = lower_level + width;
                let suggestion = format!(
                    "Expected indentation to be a multiple of {} spaces. Use {} or {} spaces instead of {}",
                    width, lower_level, higher_level, current_indent
                );
                let context =
                    crate::error::ErrorContext::from_input(&self.input, &self.position, 4)
                        .with_suggestion(suggestion);
                return Err(Error::indentation_with_context(
                    self.position,
                    (current_indent / width) * width, // expected (nearest valid level)
                    current_indent,                   // found
                    context,
                ));
            }
        }
        Ok(())
    }

    /// Check if current position starts a plain scalar
    fn is_plain_scalar_start(&self) -> bool {
        self.current_char.map_or(false, |ch| match ch {
            // Pure indicators — never start a plain scalar.
            ',' | '[' | ']' | '{' | '}' | '#' | '&' | '*' | '!' | '|' | '>' | '\'' | '"' | '%'
            | '@' | '`' => false,
            // YAML 1.2 §7.3.3: `?`, `:`, `-` may start a plain scalar when
            // the next character is non-whitespace (and, in flow context,
            // not a flow indicator). Otherwise they act as indicators
            // (complex-key marker / value separator / block-entry marker).
            '?' | ':' | '-' => match self.peek_char(1) {
                None => false,
                Some(c) if c.is_whitespace() => false,
                Some(c) if self.flow_level > 0 && ",[]{}".contains(c) => false,
                Some(_) => true,
            },
            _ => !ch.is_whitespace(),
        })
    }

    /// Check if the value is a YAML boolean
    fn is_yaml_bool(value: &str) -> bool {
        matches!(
            value,
            "true"
                | "false"
                | "True"
                | "False"
                | "TRUE"
                | "FALSE"
                | "yes"
                | "no"
                | "Yes"
                | "No"
                | "YES"
                | "NO"
                | "on"
                | "off"
                | "On"
                | "Off"
                | "ON"
                | "OFF"
        )
    }

    /// Check if the value is a YAML null
    fn is_yaml_null(value: &str) -> bool {
        matches!(value, "null" | "Null" | "NULL" | "~" | "")
    }

    /// Normalize a scalar value based on YAML rules.
    ///
    /// The scanner preserves the original text of plain scalars. Type
    /// resolution (including version-aware bool/null mapping) happens in
    /// the composer (see `crate::resolver::resolve_plain_scalar`). This
    /// preserves enough information for the composer to apply the
    /// YAML 1.1 vs 1.2 distinction and for round-trip emitters to
    /// recover the original spelling.
    fn normalize_scalar(value: String) -> String {
        value
    }

    /// Scan a number token
    fn scan_number(&mut self) -> Result<Token> {
        let start_pos = self.position;
        let mut value = String::new();

        // Handle negative numbers
        if self.current_char == Some('-') {
            value.push('-');
            self.advance();
        }

        // Scan digits
        while let Some(ch) = self.current_char {
            if ch.is_ascii_digit() {
                value.push(ch);
                self.advance();
            } else if ch == '.' {
                value.push(ch);
                self.advance();
                // Scan fractional part
                while let Some(ch) = self.current_char {
                    if ch.is_ascii_digit() {
                        value.push(ch);
                        self.advance();
                    } else {
                        break;
                    }
                }
                break;
            } else {
                break;
            }
        }

        Ok(Token::new(
            TokenType::Scalar(value, tokens::QuoteStyle::Plain),
            start_pos,
            self.position,
        ))
    }

    /// Scan a plain scalar (unquoted string)
    fn scan_plain_scalar(&mut self) -> Result<Token> {
        let start_pos = self.position;
        let start_col = start_pos.column;
        let mut value = String::new();

        loop {
            // Scan content on the current line until we hit a stop condition.
            while let Some(ch) = self.current_char {
                if self.flow_level == 0 {
                    match ch {
                        '\n' | '\r' => break,
                        ':' if self.peek_char(1).map_or(true, |c| c.is_whitespace()) => break,
                        '#' if value.is_empty()
                            || self.peek_char(-1).map_or(false, |c| c.is_whitespace()) =>
                        {
                            break;
                        }
                        _ => {}
                    }
                } else {
                    match ch {
                        ',' | '[' | ']' | '{' | '}' => break,
                        // In flow context, `:` is a key-value separator
                        // when followed by whitespace OR any flow indicator
                        // (`,`, `[`, `]`, `{`, `}`). Tracked by yaml-test-
                        // suite FRK4 (`{ ? foo :, ... }`).
                        ':' if self
                            .peek_char(1)
                            .map_or(true, |c| c.is_whitespace() || ",[]{}".contains(c)) =>
                        {
                            break;
                        }
                        '#' if value.is_empty()
                            || self.peek_char(-1).map_or(false, |c| c.is_whitespace()) =>
                        {
                            break;
                        }
                        _ => {}
                    }
                }
                value.push(ch);
                self.advance();
            }

            // If we didn't stop at a newline, this scalar is complete.
            if !matches!(self.current_char, Some('\n' | '\r')) {
                break;
            }

            // YAML 1.2 §6.5 / §7.3.3: try to fold continuation lines into
            // the same plain scalar. A continuation line must be:
            //   * indented strictly more than the scalar's start column,
            //   * not a document marker (`---` / `...`),
            //   * not a comment-only line,
            //   * not empty-with-EOF.
            // Save state for backtracking if continuation isn't allowed.
            let saved_position = self.position;
            let saved_index = self.current_char_index;
            let saved_char = self.current_char;

            // Count physical newlines we skip; whitespace within the lines
            // is also consumed.
            let mut newlines = 0usize;
            loop {
                match self.current_char {
                    Some('\n') => {
                        newlines += 1;
                        self.advance();
                    }
                    Some('\r') => {
                        self.advance();
                    }
                    Some(' ' | '\t') => {
                        self.advance();
                    }
                    _ => break,
                }
            }

            let next_col = self.position.column;
            let next_ch = self.current_char;
            let is_doc_marker = matches!(next_ch, Some('-') | Some('.'))
                && self.peek_char(1) == next_ch
                && self.peek_char(2) == next_ch
                && self.peek_char(3).map_or(true, |c| c.is_whitespace());

            // Continuation column must be >= the scalar's start column.
            // `>=` (not `>`) is correct for plain scalars at line head:
            // `---word1\nword2` (both at col 1) is a single folded scalar.
            // For scalars indented as a mapping value (e.g. `k: v1\n  v2`),
            // start_col will be the value's column and the check still
            // requires the continuation to be at least that deep.
            let can_continue = next_ch.is_some()
                && !matches!(next_ch, Some('\n' | '\r' | '#'))
                && next_col >= start_col
                && !is_doc_marker;

            if !can_continue {
                self.position = saved_position;
                self.current_char_index = saved_index;
                self.current_char = saved_char;
                break;
            }

            // Append fold separator: single newline → space; N>1 newlines
            // collapse to N-1 retained newlines (YAML §6.5 line folding).
            if newlines <= 1 {
                value.push(' ');
            } else {
                for _ in 0..(newlines - 1) {
                    value.push('\n');
                }
            }
        }

        self.resource_tracker
            .check_string_length(&self.limits, value.len())?;

        let value = value.trim_end().to_string();
        let normalized_value = Self::normalize_scalar(value);

        Ok(Token::new(
            TokenType::Scalar(normalized_value, tokens::QuoteStyle::Plain),
            start_pos,
            self.position,
        ))
    }

    /// Scan a quoted string
    fn scan_quoted_string(&mut self, quote_char: char) -> Result<Token> {
        let start_pos = self.position;
        let mut value = String::new();

        // Determine quote style based on quote character
        let quote_style = match quote_char {
            '\'' => tokens::QuoteStyle::Single,
            '"' => tokens::QuoteStyle::Double,
            _ => tokens::QuoteStyle::Plain,
        };

        self.advance(); // Skip opening quote
        let mut closed = false;

        while let Some(ch) = self.current_char {
            if ch == quote_char {
                // YAML 1.2 §7.3.2 (Single-Quoted): `''` is the only escape,
                // collapsing to a single `'`. Detect that here BEFORE
                // treating the quote as the closing delimiter.
                if quote_char == '\'' && self.peek_char(1) == Some('\'') {
                    value.push('\'');
                    self.advance();
                    self.advance();
                    continue;
                }
                self.advance(); // Skip closing quote
                closed = true;
                break;
            } else if ch == '\\' && quote_char == '"' {
                self.advance();
                if let Some(escaped) = self.current_char {
                    match escaped {
                        // YAML 1.2 §5.7 double-quoted escape allowlist.
                        'n' => value.push('\n'),
                        't' => value.push('\t'),
                        'r' => value.push('\r'),
                        '\\' => value.push('\\'),
                        '"' => value.push('"'),
                        '0' => value.push('\0'),
                        'a' => value.push('\x07'),
                        'b' => value.push('\x08'),
                        'f' => value.push('\x0C'),
                        'v' => value.push('\x0B'),
                        'e' => value.push('\x1B'),
                        ' ' => value.push(' '),
                        '/' => value.push('/'),
                        'N' => value.push('\u{0085}'),
                        '_' => value.push('\u{00A0}'),
                        'L' => value.push('\u{2028}'),
                        'P' => value.push('\u{2029}'),
                        '\n' => {
                            // Escaped line break (§7.3.2): the newline is
                            // dropped AND leading whitespace on the next
                            // line is excluded from the content.
                            self.advance();
                            while matches!(self.current_char, Some(' ' | '\t')) {
                                self.advance();
                            }
                            continue;
                        }
                        '\t' => value.push('\t'), // literal tab after `\` → tab (yaml-test-suite 3RLN/DE56)
                        // Hex / Unicode escapes per YAML 1.2 §5.7:
                        //   \xNN     — 2 hex digits, codepoint  ≤ 0xFF
                        //   \uNNNN   — 4 hex digits, codepoint  ≤ 0xFFFF
                        //   \UNNNNNNNN — 8 hex digits, full Unicode codepoint
                        'x' | 'u' | 'U' => {
                            let n = match escaped {
                                'x' => 2,
                                'u' => 4,
                                _ => 8,
                            };
                            self.advance(); // consume the x/u/U
                            let mut codepoint: u32 = 0;
                            for _ in 0..n {
                                let c = self.current_char.ok_or_else(|| {
                                    Error::scan(
                                        self.position,
                                        format!("Truncated \\{escaped} escape"),
                                    )
                                })?;
                                let d = c.to_digit(16).ok_or_else(|| {
                                    Error::scan(
                                        self.position,
                                        format!("Invalid hex digit `{c}` in \\{escaped} escape"),
                                    )
                                })?;
                                codepoint = (codepoint << 4) | d;
                                self.advance();
                            }
                            let ch = char::from_u32(codepoint).ok_or_else(|| {
                                Error::scan(
                                    self.position,
                                    format!("Invalid Unicode codepoint U+{codepoint:X}"),
                                )
                            })?;
                            value.push(ch);
                            continue; // already advanced past hex digits
                        }
                        // Everything else is invalid per spec.
                        _ => {
                            return Err(Error::scan(
                                self.position,
                                format!("Invalid escape sequence: \\{escaped}"),
                            ));
                        }
                    }
                    self.advance();
                }
            } else if ch == '\\' {
                // Single-quoted strings have no backslash escapes — `\` is
                // a literal character. (Single-quote escape is `''`.)
                value.push(ch);
                self.advance();
            } else if ch == '\n' || ch == '\r' {
                // YAML 1.2 §7.3.2 (double-quoted) / §7.3.3 (single-quoted)
                // line folding: a single newline within a quoted scalar
                // folds to a space; N>1 consecutive newlines retain N-1;
                // leading whitespace on the continuation line is excluded.
                let mut newlines = 0usize;
                while let Some(c) = self.current_char {
                    match c {
                        '\n' => {
                            newlines += 1;
                            self.advance();
                        }
                        '\r' => {
                            self.advance();
                        }
                        ' ' | '\t' => {
                            self.advance();
                        }
                        _ => break,
                    }
                }
                // Drop trailing whitespace on the prior line (the bytes
                // we already pushed) before applying the fold.
                while matches!(value.chars().last(), Some(' ' | '\t')) {
                    value.pop();
                }
                if newlines <= 1 {
                    value.push(' ');
                } else {
                    for _ in 0..(newlines - 1) {
                        value.push('\n');
                    }
                }
            } else {
                value.push(ch);
                self.advance();

                // Check string length periodically to fail fast
                if value.len() > self.limits.max_string_length {
                    return Err(Error::limit_exceeded(format!(
                        "String length {} exceeds maximum {}",
                        value.len(),
                        self.limits.max_string_length
                    )));
                }
            }
        }

        // Check string length limit
        if !closed {
            return Err(Error::scan(
                self.position,
                format!("Unclosed {} quoted string", if quote_char == '"' { "double" } else { "single" }),
            ));
        }

        self.resource_tracker
            .check_string_length(&self.limits, value.len())?;

        // YAML 1.2 §7.3.1 / §7.3.2: after the closing quote, the rest of
        // the line (or sub-expression in flow context) must be empty save
        // for a separator. Skip horizontal whitespace and look at the next
        // non-space char; if it's content rather than `,`/`:`/`}`/`]`/`#`/
        // newline/EOF, it's a trailing-content error (yaml-test-suite
        // Q4CL: `"quoted2" trailing content`).
        {
            let mut offset = 0isize;
            let mut saw_space = false;
            while matches!(self.peek_char(offset), Some(' ' | '\t')) {
                saw_space = true;
                offset += 1;
            }
            let next = self.peek_char(offset);
            // A `#` is a comment indicator ONLY when preceded by whitespace
            // (YAML 1.2 §6.6); `"value"#cmt` is invalid.
            let ok = match next {
                None => true,
                Some('#') => saw_space,
                Some(c) => matches!(c, ',' | ':' | '}' | ']' | '\n' | '\r'),
            };
            if !ok {
                return Err(Error::scan(
                    self.position,
                    format!("Unexpected `{}` after quoted scalar", next.unwrap_or(' ')),
                ));
            }
        }

        Ok(Token::new(
            TokenType::Scalar(value, quote_style),
            start_pos,
            self.position,
        ))
    }

    /// Scan document start marker (---)
    fn scan_document_start(&mut self) -> Result<Option<Token>> {
        if self.current_char == Some('-')
            && self.peek_char(1) == Some('-')
            && self.peek_char(2) == Some('-')
            && self.peek_char(3).map_or(true, |c| c.is_whitespace())
        {
            // Doc markers are invalid inside flow collections.
            if self.flow_level > 0 {
                return Err(Error::scan(
                    self.position,
                    "`---` document-start marker is not allowed inside a flow collection".to_string(),
                ));
            }
            let start_pos = self.position;
            self.advance(); // -
            self.advance(); // -
            self.advance(); // -

            Ok(Some(Token::new(
                TokenType::DocumentStart,
                start_pos,
                self.position,
            )))
        } else {
            Ok(None)
        }
    }

    /// Scan YAML version directive (%YAML)
    fn scan_yaml_directive(&mut self) -> Result<Option<Token>> {
        if self.current_char != Some('%') {
            return Ok(None);
        }

        let start_pos = self.position;
        let saved_position = self.position;
        self.advance(); // Skip '%'

        // Check for "YAML"
        if self.current_char == Some('Y')
            && self.peek_char(1) == Some('A')
            && self.peek_char(2) == Some('M')
            && self.peek_char(3) == Some('L')
            && self.peek_char(4).map_or(false, |c| c.is_whitespace())
        {
            self.advance(); // Y
            self.advance(); // A
            self.advance(); // M
            self.advance(); // L

            // Skip whitespace
            self.skip_whitespace();

            // Parse version number (e.g., "1.2")
            let major = if let Some(ch) = self.current_char {
                if ch.is_ascii_digit() {
                    let digit = ch.to_digit(10).unwrap() as u8;
                    self.advance();
                    digit
                } else {
                    return Err(Error::scan(
                        self.position,
                        "Expected major version number after %YAML".to_string(),
                    ));
                }
            } else {
                return Err(Error::scan(
                    self.position,
                    "Expected version after %YAML directive".to_string(),
                ));
            };

            // Expect '.'
            if self.current_char != Some('.') {
                return Err(Error::scan(
                    self.position,
                    "Expected '.' in YAML version".to_string(),
                ));
            }
            self.advance();

            // Parse minor version
            let minor = if let Some(ch) = self.current_char {
                if ch.is_ascii_digit() {
                    let digit = ch.to_digit(10).unwrap() as u8;
                    self.advance();
                    digit
                } else {
                    return Err(Error::scan(
                        self.position,
                        "Expected minor version number after '.'".to_string(),
                    ));
                }
            } else {
                return Err(Error::scan(
                    self.position,
                    "Expected minor version number".to_string(),
                ));
            };

            // YAML 1.2 §6.8.1: the directive line must end after the
            // version (modulo whitespace and an optional comment). Extra
            // tokens (e.g. `%YAML 1.2 foo`) are invalid — yaml-test-suite
            // H7TQ. Also `%YAML 1.1#...` (yaml-test-suite MUS6/00) needs
            // whitespace before `#`.
            let mut saw_space = false;
            while matches!(self.current_char, Some(' ' | '\t')) {
                saw_space = true;
                self.advance();
            }
            match self.current_char {
                None | Some('\n' | '\r') => {}
                Some('#') if saw_space => {
                    while let Some(ch) = self.current_char {
                        if ch == '\n' || ch == '\r' {
                            break;
                        }
                        self.advance();
                    }
                }
                Some(c) => {
                    return Err(Error::scan(
                        self.position,
                        format!("Unexpected `{c}` after %YAML directive"),
                    ));
                }
            }

            Ok(Some(Token::new(
                TokenType::YamlDirective(major, minor),
                start_pos,
                self.position,
            )))
        } else {
            // Not a YAML directive, reset position
            self.position = saved_position;
            // Properly reset current_char based on saved position
            self.current_char = self
                .char_indices
                .iter()
                .find(|(i, _)| *i == saved_position.index)
                .map(|(_, ch)| *ch);
            // Reset the current_char_index
            self.current_char_index = self
                .char_indices
                .iter()
                .position(|(i, _)| *i == saved_position.index)
                .unwrap_or(0);
            Ok(None)
        }
    }

    /// Scan TAG directive (%TAG)
    fn scan_tag_directive(&mut self) -> Result<Option<Token>> {
        if self.current_char != Some('%') {
            return Ok(None);
        }

        let start_pos = self.position;
        let saved_position = self.position;
        self.advance(); // Skip '%'

        // Check for "TAG"
        if self.current_char == Some('T')
            && self.peek_char(1) == Some('A')
            && self.peek_char(2) == Some('G')
            && self.peek_char(3).map_or(false, |c| c.is_whitespace())
        {
            self.advance(); // T
            self.advance(); // A
            self.advance(); // G

            // Skip whitespace
            self.skip_whitespace();

            // Parse handle (e.g., "!" or "!!")
            let handle = self.scan_tag_handle()?;

            // Skip whitespace
            self.skip_whitespace();

            // Parse prefix (URI)
            let prefix = self.scan_tag_prefix()?;

            Ok(Some(Token::new(
                TokenType::TagDirective(handle, prefix),
                start_pos,
                self.position,
            )))
        } else {
            // Reset position if not a TAG directive
            self.position = saved_position;
            // Properly reset current_char based on saved position
            self.current_char = self
                .char_indices
                .iter()
                .find(|(i, _)| *i == saved_position.index)
                .map(|(_, ch)| *ch);
            // Reset the current_char_index
            self.current_char_index = self
                .char_indices
                .iter()
                .position(|(i, _)| *i == saved_position.index)
                .unwrap_or(0);
            Ok(None)
        }
    }

    /// Scan a tag handle for TAG directive
    fn scan_tag_handle(&mut self) -> Result<String> {
        let mut handle = String::new();

        if self.current_char != Some('!') {
            return Err(Error::scan(
                self.position,
                "Expected '!' at start of tag handle".to_string(),
            ));
        }

        handle.push('!');
        self.advance();

        // Handle can be "!" or "!!" or "!name!"
        if self.current_char == Some('!') {
            // Secondary handle "!!"
            handle.push('!');
            self.advance();
        } else if self.current_char.map_or(false, |c| c.is_alphanumeric()) {
            // Named handle like "!name!"
            while let Some(ch) = self.current_char {
                if ch.is_alphanumeric() || ch == '-' || ch == '_' {
                    handle.push(ch);
                    self.advance();
                } else if ch == '!' {
                    handle.push(ch);
                    self.advance();
                    break;
                } else {
                    break;
                }
            }
        }
        // else just "!" primary handle

        Ok(handle)
    }

    /// Scan a tag prefix (URI) for TAG directive
    fn scan_tag_prefix(&mut self) -> Result<String> {
        let mut prefix = String::new();

        // Read until end of line or comment
        while let Some(ch) = self.current_char {
            if ch == '\n' || ch == '\r' || ch == '#' {
                break;
            }
            if ch.is_whitespace() && prefix.is_empty() {
                self.advance();
                continue;
            }
            if ch.is_whitespace() && !prefix.is_empty() {
                // Trailing whitespace, we're done
                break;
            }
            prefix.push(ch);
            self.advance();
        }

        if prefix.is_empty() {
            return Err(Error::scan(
                self.position,
                "Expected tag prefix after tag handle".to_string(),
            ));
        }

        Ok(prefix.trim().to_string())
    }

    /// Check if current position might be a directive
    fn is_directive(&self) -> bool {
        self.current_char == Some('%') && self.position.column == 1
    }

    /// Scan document end marker (...)
    fn scan_document_end(&mut self) -> Result<Option<Token>> {
        if self.current_char == Some('.')
            && self.peek_char(1) == Some('.')
            && self.peek_char(2) == Some('.')
            && self.peek_char(3).map_or(true, |c| c.is_whitespace())
        {
            // Doc markers are invalid inside flow collections.
            if self.flow_level > 0 {
                return Err(Error::scan(
                    self.position,
                    "`...` document-end marker is not allowed inside a flow collection".to_string(),
                ));
            }
            let start_pos = self.position;
            self.advance(); // .
            self.advance(); // .
            self.advance(); // .

            // YAML 1.2 §6.4: `...` must be followed only by whitespace or
            // end-of-line (comments allowed). Inline content after `...`
            // is invalid (yaml-test-suite 3HFZ).
            while let Some(ch) = self.current_char {
                match ch {
                    ' ' | '\t' => {
                        self.advance();
                    }
                    '\n' | '\r' | '#' => break,
                    _ => {
                        return Err(Error::scan(
                            self.position,
                            "Content after `...` document-end marker is invalid".to_string(),
                        ));
                    }
                }
            }

            Ok(Some(Token::new(
                TokenType::DocumentEnd,
                start_pos,
                self.position,
            )))
        } else {
            Ok(None)
        }
    }

    /// Scan a comment token
    fn scan_comment(&mut self) -> Result<Token> {
        let start_pos = self.position;
        let mut comment_text = String::new();

        // Skip the '#' character
        if self.current_char == Some('#') {
            self.advance();
        }

        // Collect the comment text
        while let Some(ch) = self.current_char {
            if ch == '\n' || ch == '\r' {
                break;
            }
            comment_text.push(ch);
            self.advance();
        }

        // Trim leading whitespace from comment text
        let comment_text = comment_text.trim_start().to_string();

        Ok(Token::new(
            TokenType::Comment(comment_text),
            start_pos,
            self.position,
        ))
    }

    /// Process a line and generate appropriate tokens
    #[allow(clippy::cognitive_complexity)]
    fn process_line(&mut self) -> Result<()> {
        // Check for directives at start of line
        if self.position.column == 1 && self.current_char == Some('%') {
            // Try to scan YAML directive
            if let Some(token) = self.scan_yaml_directive()? {
                self.tokens.push(token);
                return Ok(());
            }

            // Try to scan TAG directive
            if let Some(token) = self.scan_tag_directive()? {
                self.tokens.push(token);
                return Ok(());
            }

            // YAML 1.2 §6.8.4: a YAML processor MUST ignore directives it
            // does not recognize. Skip the line silently — parsing continues
            // with whatever follows on the next line.
            if self.current_char == Some('%') {
                while let Some(ch) = self.current_char {
                    if ch == '\n' || ch == '\r' {
                        break;
                    }
                    self.advance();
                }
                return Ok(());
            }
        }

        // Check for document markers at start of line
        if self.position.column == 1 {
            // Check for document start marker
            if let Some(token) = self.scan_document_start()? {
                self.tokens.push(token);
                return Ok(());
            }

            // Check for document end marker
            if let Some(token) = self.scan_document_end()? {
                self.tokens.push(token);
                return Ok(());
            }
        }

        // Handle indentation at start of line
        if self.position.column == 1 {
            self.handle_indentation()?;
        }

        // Skip empty lines and comments
        self.skip_whitespace();

        match self.current_char {
            None => return Ok(()),
            Some('#') => {
                if self.preserve_comments {
                    // Create a comment token
                    let comment_token = self.scan_comment()?;
                    self.tokens.push(comment_token);
                } else {
                    // Skip comment lines
                    while let Some(ch) = self.current_char {
                        if ch == '\n' || ch == '\r' {
                            break;
                        }
                        self.advance();
                    }
                }
                return Ok(());
            }
            Some('\n' | '\r') => {
                self.advance();
                return Ok(());
            }
            _ => {}
        }

        // Process tokens on this line
        while let Some(ch) = self.current_char {
            match ch {
                '\n' | '\r' => break,
                ' ' | '\t' => {
                    self.skip_whitespace();
                }
                '#' => {
                    // YAML 1.2 §6.6: a comment must be preceded by whitespace
                    // OR be at the start of a line. Inputs like `,#invalid`
                    // (yaml-test-suite CVW2) are not valid comments.
                    let prev = self.peek_char(-1);
                    let at_line_start = self.position.column == 1;
                    let preceded_by_space = prev.map_or(true, |c| c.is_whitespace());
                    if !at_line_start && !preceded_by_space {
                        return Err(Error::scan(
                            self.position,
                            "Comment `#` must be preceded by whitespace".to_string(),
                        ));
                    }
                    if self.preserve_comments {
                        let comment_token = self.scan_comment()?;
                        self.tokens.push(comment_token);
                    } else {
                        while let Some(ch) = self.current_char {
                            if ch == '\n' || ch == '\r' {
                                break;
                            }
                            self.advance();
                        }
                    }
                    break;
                }

                // Flow indicators
                '[' => {
                    let pos = self.position;
                    self.advance();
                    self.flow_level += 1;
                    // Check depth limit
                    self.resource_tracker
                        .check_depth(&self.limits, self.flow_level + self.indent_stack.len())?;
                    self.tokens
                        .push(Token::new(TokenType::FlowSequenceStart, pos, self.position));
                }
                ']' => {
                    let pos = self.position;
                    self.advance();
                    if self.flow_level > 0 {
                        self.flow_level -= 1;
                    }
                    self.tokens
                        .push(Token::new(TokenType::FlowSequenceEnd, pos, self.position));
                }
                '{' => {
                    let pos = self.position;
                    self.advance();
                    self.flow_level += 1;
                    // Check depth limit
                    self.resource_tracker
                        .check_depth(&self.limits, self.flow_level + self.indent_stack.len())?;
                    self.tokens
                        .push(Token::new(TokenType::FlowMappingStart, pos, self.position));
                }
                '}' => {
                    let pos = self.position;
                    self.advance();
                    if self.flow_level > 0 {
                        self.flow_level -= 1;
                    }
                    self.tokens
                        .push(Token::new(TokenType::FlowMappingEnd, pos, self.position));
                }
                ',' => {
                    let pos = self.position;
                    self.advance();
                    self.tokens
                        .push(Token::new(TokenType::FlowEntry, pos, self.position));
                }

                // Key-value separator. YAML 1.2 §7.3.3 / §7.4:
                //   * Block context: `:` separates key from value only when
                //     followed by whitespace / EOF — otherwise it's part of
                //     a plain scalar (e.g. `:foo`, `URL://path`).
                //   * Flow context: same, plus `:` may be adjacent to a
                //     value when the previous token completed a key node
                //     (quoted/plain scalar, alias, or closed flow
                //     collection) — see yaml-test-suite 5MUD, 5T43.
                ':' if self
                    .peek_char(1)
                    .map_or(true, |c| {
                        c.is_whitespace() || (self.flow_level > 0 && ",[]{}".contains(c))
                    })
                    || (self.flow_level > 0
                        && matches!(
                            self.tokens.last().map(|t| &t.token_type),
                            Some(
                                TokenType::Scalar(_, _)
                                    | TokenType::Alias(_)
                                    | TokenType::FlowMappingEnd
                                    | TokenType::FlowSequenceEnd
                            )
                        )) =>
                {
                    let pos = self.position;
                    self.advance();
                    self.tokens
                        .push(Token::new(TokenType::Value, pos, self.position));
                }

                // Explicit key marker
                '?' if self.flow_level == 0
                    && (self.peek_char(1).map_or(true, |c| c.is_whitespace())
                        || self.peek_char(1).is_none()) =>
                {
                    let pos = self.position;
                    self.advance();
                    self.tokens
                        .push(Token::new(TokenType::Key, pos, self.position));
                }
                '?' if self.flow_level > 0
                    && (self
                        .peek_char(1)
                        .map_or(true, |c| c.is_whitespace() || ",:]}".contains(c))
                        || self.peek_char(1).is_none()) =>
                {
                    let pos = self.position;
                    self.advance();
                    self.tokens
                        .push(Token::new(TokenType::Key, pos, self.position));
                }

                // Block entry
                '-' if self.flow_level == 0
                    && (self.peek_char(1).map_or(true, |c| c.is_whitespace())
                        || self.peek_char(1).is_none()) =>
                {
                    let pos = self.position;
                    self.advance();

                    // Check if we need to start a new block sequence
                    let last_indent = *self.indent_stack.last().unwrap();

                    if self.current_indent > last_indent {
                        // Deeper indentation - start new nested sequence
                        self.indent_stack.push(self.current_indent);
                        self.indent_is_sequence.push(true);
                        // Check depth limit
                        self.resource_tracker
                            .check_depth(&self.limits, self.flow_level + self.indent_stack.len())?;
                        self.tokens
                            .push(Token::simple(TokenType::BlockSequenceStart, pos));
                    } else if self.current_indent == last_indent
                        && *self.indent_is_sequence.last().unwrap_or(&false)
                    {
                        // Same indent and the top of stack is already a sequence
                        // → continuation of that sequence; no new start needed.
                    } else if self.current_indent >= last_indent {
                        // Same or root level — compact notation.
                        // Start a new sequence only if we don't already have one
                        // tracked at this exact indent.
                        let has_active_compact = self
                            .compact_sequence_indents
                            .last()
                            .map_or(false, |&si| si == self.current_indent);

                        if !has_active_compact {
                            self.compact_sequence_indents.push(self.current_indent);
                            // Check depth limit
                            self.resource_tracker.check_depth(
                                &self.limits,
                                self.flow_level + self.indent_stack.len(),
                            )?;
                            self.tokens
                                .push(Token::simple(TokenType::BlockSequenceStart, pos));
                        }
                    }

                    self.tokens
                        .push(Token::new(TokenType::BlockEntry, pos, self.position));

                    // After emitting BlockEntry, check if the next token is another dash (nested sequence)
                    self.skip_whitespace();
                    if self.current_char == Some('-')
                        && self.peek_char(1).map_or(true, |c| c.is_whitespace())
                    {
                        // We have a nested sequence on the same line!
                        // Track this as an inline sequence
                        self.inline_sequence_depth += 1;
                        // Also push to indent_stack to track proper nesting
                        self.indent_stack.push(self.position.column);
                        self.indent_is_sequence.push(true);
                        // Check depth limit
                        self.resource_tracker
                            .check_depth(&self.limits, self.flow_level + self.indent_stack.len())?;
                        self.tokens
                            .push(Token::simple(TokenType::BlockSequenceStart, self.position));
                        // Continue processing - the next iteration will handle the nested dash
                    } else if self.current_char.is_some()
                        && !matches!(self.current_char, Some('\n' | '\r'))
                    {
                        // Content follows "- " on the same line.
                        // Update current_indent to the content's column position so that
                        // any mapping started here will be at a deeper indent level than
                        // the sequence. This ensures handle_indentation properly closes
                        // the mapping when the next sibling "- " appears.
                        self.current_indent = self.position.column - 1;
                    }
                }

                // Quoted strings
                '"' => {
                    let token = self.scan_quoted_string('"')?;
                    self.tokens.push(token);
                }
                '\'' => {
                    let token = self.scan_quoted_string('\'')?;
                    self.tokens.push(token);
                }

                // Document markers (only if not a block entry).
                //
                // Reached only when `-` is at column = current_indent + 1 AND
                // the next character is non-whitespace — i.e. either the
                // `---` document-start marker OR a plain scalar starting
                // with `-` (e.g. `---word1`, `-foo`). If `scan_document_start`
                // declines, we MUST consume the run as a plain scalar — not
                // consulting `is_plain_scalar_start` here, because that helper
                // unconditionally rejects `-`, which would leave the outer
                // `while let` loop spinning on the same character.
                '-' if self.position.column == self.current_indent + 1
                    && !self.peek_char(1).map_or(true, |c| c.is_whitespace()) =>
                {
                    if let Some(token) = self.scan_document_start()? {
                        self.tokens.push(token);
                    } else {
                        let token = self.scan_plain_scalar()?;
                        self.tokens.push(token);
                    }
                }
                '.' if self.position.column == self.current_indent + 1 => {
                    if let Some(token) = self.scan_document_end()? {
                        self.tokens.push(token);
                    } else if self.is_plain_scalar_start() {
                        let token = self.scan_plain_scalar()?;
                        self.tokens.push(token);
                    }
                }

                // Numbers or plain scalars starting with -
                // Only scan as number if the entire token is numeric (no trailing letters)
                _ if (ch.is_ascii_digit()
                    || (ch == '-' && self.peek_char(1).map_or(false, |c| c.is_ascii_digit())))
                    && self.is_pure_number() =>
                {
                    let token = self.scan_number()?;
                    self.tokens.push(token);
                }

                // Anchors and aliases
                '&' => {
                    let token = self.scan_anchor()?;
                    self.tokens.push(token);
                }
                '*' => {
                    let token = self.scan_alias()?;
                    self.tokens.push(token);
                }

                // Block scalars
                '|' => {
                    let token = self.scan_literal_block_scalar()?;
                    self.tokens.push(token);
                }
                '>' => {
                    let token = self.scan_folded_block_scalar()?;
                    self.tokens.push(token);
                }

                // Tags
                '!' => {
                    let token = self.scan_tag()?;
                    self.tokens.push(token);
                }

                // Plain scalars
                _ if self.is_plain_scalar_start() => {
                    // Look ahead to see if this is a mapping key
                    if self.flow_level == 0 {
                        let should_start_mapping = self.check_for_mapping_ahead();
                        if should_start_mapping {
                            let last_indent = *self.indent_stack.last().unwrap();

                            // Check if we should start a new mapping
                            // Start a mapping if:
                            // 1. No mapping is active at this indentation level, OR
                            // 2. We're at a deeper indentation level (nested mapping)
                            let should_start_new_mapping = if self.current_indent > last_indent {
                                // Deeper indentation - start nested mapping
                                true
                            } else if self.current_indent == last_indent {
                                // Same indentation - check if there's an active mapping at this level
                                // We need to carefully track mapping contexts across BlockEnd tokens
                                let has_active_mapping_at_this_level =
                                    self.check_active_mapping_at_level(self.current_indent);
                                !has_active_mapping_at_this_level
                            } else {
                                // Shallower indentation - should have been handled by handle_indentation
                                false
                            };

                            if should_start_new_mapping {
                                // Start mapping before processing the key
                                self.indent_stack.push(self.current_indent);
                                self.indent_is_sequence.push(false);
                                // Check depth limit
                                self.resource_tracker.check_depth(
                                    &self.limits,
                                    self.flow_level + self.indent_stack.len(),
                                )?;
                                self.tokens.push(Token::simple(
                                    TokenType::BlockMappingStart,
                                    self.position,
                                ));
                            }
                        }
                    }

                    let token = self.scan_plain_scalar()?;
                    self.tokens.push(token);
                }

                _ => {
                    let context = ErrorContext::from_input(&self.input, &self.position, 2)
                        .with_suggestion("Check for valid YAML syntax characters".to_string());
                    return Err(Error::invalid_character_with_context(
                        self.position,
                        ch,
                        "YAML document",
                        context,
                    ));
                }
            }
        }

        // After processing the line, close any inline sequences
        while self.inline_sequence_depth > 0 {
            self.inline_sequence_depth -= 1;
            // Also pop from indent_stack
            if self.indent_stack.len() > 1 {
                self.indent_stack.pop();
                self.indent_is_sequence.pop();
            }
            self.tokens
                .push(Token::simple(TokenType::BlockEnd, self.position));
        }

        Ok(())
    }

    /// Scan the next token lazily
    fn scan_next_token(&mut self) -> Result<()> {
        if self.done {
            return Ok(());
        }

        // Add stream start token if this is the beginning
        if self.tokens.is_empty() {
            self.tokens
                .push(Token::simple(TokenType::StreamStart, self.position));
            return Ok(());
        }

        // Check if we're at the end of input
        if self.current_char.is_none() {
            if !self
                .tokens
                .iter()
                .any(|t| matches!(t.token_type, TokenType::StreamEnd))
            {
                self.tokens
                    .push(Token::simple(TokenType::StreamEnd, self.position));
            }
            self.done = true;
            return Ok(());
        }

        // For now, fall back to scanning all tokens at once for the lazy scanner
        // This is a simplified implementation - a full streaming parser would
        // need more sophisticated state management
        let tokens_before = self.tokens.len();
        self.scan_all_tokens()?;

        // Mark as done after scanning all tokens
        if self.tokens.len() == tokens_before {
            self.done = true;
        }

        Ok(())
    }

    /// Pre-scan all tokens (simplified approach for basic implementation)
    fn scan_all_tokens(&mut self) -> Result<()> {
        // Only add StreamStart if we don't have it yet
        if !self
            .tokens
            .iter()
            .any(|t| matches!(t.token_type, TokenType::StreamStart))
        {
            self.tokens
                .push(Token::simple(TokenType::StreamStart, self.position));
        }

        while self.current_char.is_some() {
            self.process_line()?;

            // Advance past newlines
            while let Some(ch) = self.current_char {
                if ch == '\n' || ch == '\r' {
                    self.advance();
                } else {
                    break;
                }
            }
        }

        // Close any remaining compact sequences (before their parent mappings)
        while self.compact_sequence_indents.pop().is_some() {
            self.tokens
                .push(Token::simple(TokenType::BlockEnd, self.position));
        }

        // Close any remaining blocks
        while self.indent_stack.len() > 1 {
            self.indent_stack.pop();
            self.indent_is_sequence.pop();
            self.tokens
                .push(Token::simple(TokenType::BlockEnd, self.position));
        }

        self.tokens
            .push(Token::simple(TokenType::StreamEnd, self.position));
        self.done = true;
        Ok(())
    }

    /// Peek at a character at the given offset (can be negative)
    /// Check if the current position starts a pure number (digits/dots/minus only,
    /// not followed by letters). Values like 500m, 128Mi should be treated as plain scalars.
    fn is_pure_number(&self) -> bool {
        let mut offset: isize = 0;
        let first = self.peek_char(0);
        // Skip leading minus
        if first == Some('-') {
            offset = 1;
        }
        // Scan digits and at most one dot
        let mut has_digit = false;
        let mut dot_count = 0;
        loop {
            match self.peek_char(offset) {
                Some(c) if c.is_ascii_digit() => {
                    has_digit = true;
                    offset += 1;
                }
                Some('.') => {
                    dot_count += 1;
                    if dot_count > 1 {
                        // Multiple dots (e.g. 0.5.8) — not a number
                        return false;
                    }
                    offset += 1;
                }
                Some(c) if c.is_ascii_alphabetic() || c == '_' => {
                    // Letters follow the digits — not a pure number (e.g. 500m, 128Mi)
                    return false;
                }
                _ => {
                    // Whitespace, newline, colon, EOF, etc. — end of token
                    return has_digit;
                }
            }
        }
    }

    fn peek_char(&self, offset: isize) -> Option<char> {
        if offset >= 0 {
            let target_index = self.current_char_index + offset as usize;
            if target_index < self.char_cache.len() {
                Some(self.char_cache[target_index])
            } else {
                None
            }
        } else {
            let offset_magnitude = (-offset) as usize;
            if self.current_char_index >= offset_magnitude {
                Some(self.char_cache[self.current_char_index - offset_magnitude])
            } else {
                None
            }
        }
    }

    /// Scan an anchor token (&name)
    fn scan_anchor(&mut self) -> Result<Token> {
        let start_pos = self.position;
        self.advance(); // Skip '&'

        let name = self.scan_identifier()?;
        if name.is_empty() {
            let context = ErrorContext::from_input(&self.input, &self.position, 2).with_suggestion(
                "Provide a valid anchor name after &, e.g., &anchor_name".to_string(),
            );
            return Err(Error::scan_with_context(
                self.position,
                "Anchor name cannot be empty",
                context,
            ));
        }

        // Track anchor for resource limits
        self.resource_tracker.add_anchor(&self.limits)?;

        Ok(Token::new(
            TokenType::Anchor(name),
            start_pos,
            self.position,
        ))
    }

    /// Scan an alias token (*name)
    fn scan_alias(&mut self) -> Result<Token> {
        let start_pos = self.position;
        self.advance(); // Skip '*'

        let name = self.scan_identifier()?;
        if name.is_empty() {
            let context = ErrorContext::from_input(&self.input, &self.position, 2).with_suggestion(
                "Provide a valid alias name after *, e.g., *alias_name".to_string(),
            );
            return Err(Error::scan_with_context(
                self.position,
                "Alias name cannot be empty",
                context,
            ));
        }

        Ok(Token::new(TokenType::Alias(name), start_pos, self.position))
    }

    /// Scan an identifier (used for anchor and alias names)
    fn scan_identifier(&mut self) -> Result<String> {
        // Per YAML 1.2 §6.9.2 (ns-anchor-name = ns-anchor-char+), the only
        // exclusions are whitespace and the flow indicators `,[]{}`. This
        // accepts ASCII alphanumeric, underscore, hyphen, AND full unicode
        // codepoints (including emoji), matching the spec exactly.
        let mut identifier = String::new();
        while let Some(ch) = self.current_char {
            if ch.is_whitespace() || matches!(ch, ',' | '[' | ']' | '{' | '}') {
                break;
            }
            identifier.push(ch);
            self.advance();
        }
        Ok(identifier)
    }

    /// Scan a tag token (!tag or !!tag or !<verbatim>)
    fn scan_tag(&mut self) -> Result<Token> {
        let start_pos = self.position;
        self.advance(); // Skip first '!'

        let mut tag = String::from("!");

        // Check for verbatim tag format: !<tag>
        if self.current_char == Some('<') {
            tag.push('<');
            self.advance(); // Skip '<'

            // Scan until closing '>'
            while let Some(ch) = self.current_char {
                if ch == '>' {
                    tag.push(ch);
                    self.advance();
                    break;
                } else if ch.is_control() || ch.is_whitespace() {
                    return Err(Error::scan(
                        self.position,
                        "Invalid character in verbatim tag".to_string(),
                    ));
                }
                tag.push(ch);
                self.advance();
            }
        } else {
            // Check for secondary tag handle: !!
            if self.current_char == Some('!') {
                tag.push('!');
                self.advance(); // Skip second '!'
            }

            // Scan tag name/suffix.
            //
            // Per YAML 1.2 §5.6, tag suffixes are URI references — they may
            // contain any URI character (RFC 3986 unreserved + sub-delims +
            // a few others) or `%XX` percent-encoded bytes. The handful of
            // characters listed below covers the alphanumeric + URI-safe
            // punctuation set used by yaml-test-suite. Percent decoding of
            // `%XX` happens later in `TagResolver::resolve`.
            while let Some(ch) = self.current_char {
                if ch.is_alphanumeric() || "-._~:/?#[]@!$&'()*+,;=%".contains(ch) {
                    tag.push(ch);
                    self.advance();
                } else {
                    break;
                }
            }
        }

        Ok(Token::new(TokenType::Tag(tag), start_pos, self.position))
    }

    /// Scan a literal block scalar (|)
    fn scan_literal_block_scalar(&mut self) -> Result<Token> {
        let start_pos = self.position;
        self.advance(); // Skip '|'

        // Parse block scalar header (indicators like +, -, explicit indent)
        let (keep_trailing, explicit_indent) = self.scan_block_scalar_header()?;

        // Skip to next line
        self.skip_to_next_line()?;

        // Determine indentation
        let base_indent = self.current_indent;
        let content_indent = if let Some(explicit) = explicit_indent {
            base_indent + explicit
        } else {
            // Find the first non-empty content line to determine indentation
            self.find_block_scalar_indent(base_indent)?
        };

        // Collect the literal block content
        let content = self.collect_literal_block_content(content_indent, keep_trailing)?;

        Ok(Token::new(
            TokenType::BlockScalarLiteral(content),
            start_pos,
            self.position,
        ))
    }

    /// Scan a folded block scalar (>)
    fn scan_folded_block_scalar(&mut self) -> Result<Token> {
        let start_pos = self.position;
        self.advance(); // Skip '>'

        // Parse block scalar header (indicators like +, -, explicit indent)
        let (keep_trailing, explicit_indent) = self.scan_block_scalar_header()?;

        // Skip to next line
        self.skip_to_next_line()?;

        // Determine indentation
        let base_indent = self.current_indent;
        let content_indent = if let Some(explicit) = explicit_indent {
            base_indent + explicit
        } else {
            // Find the first non-empty content line to determine indentation
            self.find_block_scalar_indent(base_indent)?
        };

        // Collect the folded block content
        let content = self.collect_folded_block_content(content_indent, keep_trailing)?;

        Ok(Token::new(
            TokenType::BlockScalarFolded(content),
            start_pos,
            self.position,
        ))
    }

    /// Parse block scalar header indicators (+, -, and explicit indent)
    fn scan_block_scalar_header(&mut self) -> Result<(bool, Option<usize>)> {
        let mut keep_trailing = false;
        let mut explicit_indent: Option<usize> = None;

        // Parse indicators in any order
        while let Some(ch) = self.current_char {
            match ch {
                '+' => {
                    keep_trailing = true;
                    self.advance();
                }
                '-' => {
                    keep_trailing = false; // Strip trailing newlines
                    self.advance();
                }
                '0'..='9' => {
                    let digit = ch.to_digit(10).unwrap() as usize;
                    if explicit_indent.is_some() {
                        let context = ErrorContext::from_input(&self.input, &self.position, 2)
                            .with_suggestion(
                                "Use only one indent indicator digit in block scalar".to_string(),
                            );
                        return Err(Error::scan_with_context(
                            self.position,
                            "Multiple indent indicators in block scalar",
                            context,
                        ));
                    }
                    explicit_indent = Some(digit);
                    self.advance();
                }
                ' ' | '\t' => {
                    self.advance(); // Skip whitespace
                }
                '#' => {
                    // Skip comment to end of line
                    while let Some(ch) = self.current_char {
                        self.advance();
                        if ch == '\n' || ch == '\r' {
                            break;
                        }
                    }
                    break;
                }
                '\n' | '\r' => break,
                _ => {
                    let context = ErrorContext::from_input(&self.input, &self.position, 2)
                        .with_suggestion("Use valid block scalar indicators: | (literal), > (folded), + (keep), - (strip), or digit (indent)".to_string());
                    return Err(Error::invalid_character_with_context(
                        self.position,
                        ch,
                        "block scalar header",
                        context,
                    ));
                }
            }
        }

        Ok((keep_trailing, explicit_indent))
    }

    /// Skip whitespace and comments to the next content line
    fn skip_to_next_line(&mut self) -> Result<()> {
        while let Some(ch) = self.current_char {
            match ch {
                '\n' | '\r' => {
                    self.advance();
                    break;
                }
                ' ' | '\t' => {
                    self.advance();
                }
                _ => break,
            }
        }
        Ok(())
    }

    /// Find the content indentation for a block scalar
    fn find_block_scalar_indent(&mut self, base_indent: usize) -> Result<usize> {
        let saved_position = self.position;
        let saved_char = self.current_char;
        let saved_char_index = self.current_char_index;

        let mut content_indent = base_indent + 1; // Default minimum indent

        // Look ahead to find the first non-empty line
        while let Some(ch) = self.current_char {
            self.advance();
            if ch == '\n' || ch == '\r' {
                let line_indent = self.count_line_indent();

                // If this line has content (not just whitespace)
                if let Some(line_ch) = self.current_char {
                    if line_ch != '\n' && line_ch != '\r' {
                        if line_indent > base_indent {
                            content_indent = line_indent;
                            break;
                        }
                        // Content must be indented more than the block scalar indicator
                        content_indent = base_indent + 1;
                        break;
                    }
                }
            }
        }

        // Restore position
        self.position = saved_position;
        self.current_char = saved_char;
        self.current_char_index = saved_char_index;

        Ok(content_indent)
    }

    /// Count indentation at start of current line
    fn count_line_indent(&mut self) -> usize {
        let mut indent = 0;
        let saved_position = self.position;
        let saved_char = self.current_char;
        let saved_char_index = self.current_char_index;

        while let Some(ch) = self.current_char {
            if ch == ' ' {
                indent += 1;
                self.advance();
            } else if ch == '\t' {
                indent += 8; // Tab counts as 8 spaces
                self.advance();
            } else {
                break;
            }
        }

        // Restore position
        self.position = saved_position;
        self.current_char = saved_char;
        self.current_char_index = saved_char_index;

        indent
    }

    /// Collect content for a literal block scalar
    fn collect_literal_block_content(
        &mut self,
        content_indent: usize,
        _keep_trailing: bool,
    ) -> Result<String> {
        let mut content = String::new();

        while let Some(_) = self.current_char {
            let line_indent = self.count_line_indent();

            // Skip indentation
            for _ in 0..content_indent.min(line_indent) {
                if let Some(' ' | '\t') = self.current_char {
                    self.advance();
                }
            }

            // Collect line content
            let mut line = String::new();
            while let Some(ch) = self.current_char {
                if ch == '\n' || ch == '\r' {
                    self.advance();
                    break;
                }
                line.push(ch);
                self.advance();
            }

            // Check if we've reached the end of the block scalar
            if line_indent < content_indent && !line.trim().is_empty() {
                // This line is part of the next construct
                break;
            }

            // Add line to content (preserving literal newlines)
            content.push_str(&line);
            if self.current_char.is_some() {
                content.push('\n');
            }

            // Check for end of input or document boundaries
            if self.current_char.is_none() {
                break;
            }
        }

        Ok(content)
    }

    /// Collect content for a folded block scalar
    fn collect_folded_block_content(
        &mut self,
        content_indent: usize,
        _keep_trailing: bool,
    ) -> Result<String> {
        let mut content = String::new();
        let mut prev_was_empty = false;
        let mut first_line = true;

        while let Some(_) = self.current_char {
            let line_indent = self.count_line_indent();

            // Skip indentation
            for _ in 0..content_indent.min(line_indent) {
                if let Some(' ' | '\t') = self.current_char {
                    self.advance();
                }
            }

            // Collect line content
            let mut line = String::new();
            while let Some(ch) = self.current_char {
                if ch == '\n' || ch == '\r' {
                    self.advance();
                    break;
                }
                line.push(ch);
                self.advance();
            }

            // Check if we've reached the end of the block scalar
            if line_indent < content_indent && !line.trim().is_empty() {
                break;
            }

            let line_is_empty = line.trim().is_empty();

            if line_is_empty {
                // Empty lines are preserved as-is
                if !first_line && !prev_was_empty {
                    content.push('\n');
                }
                prev_was_empty = true;
            } else {
                // Non-empty lines are folded (joined with spaces)
                if !first_line && !prev_was_empty {
                    content.push(' '); // Fold previous line break into space
                }
                content.push_str(line.trim());
                prev_was_empty = false;
            }

            first_line = false;

            if self.current_char.is_none() {
                break;
            }
        }

        Ok(content)
    }

    /// Check if the current position is the start of a mapping key by looking ahead for ':'
    fn check_for_mapping_ahead(&self) -> bool {
        // Look ahead through the current line for a ':' character
        for i in self.current_char_index..self.char_cache.len() {
            let ch = self.char_cache[i];
            match ch {
                ':' => {
                    // Found colon, check if it's followed by whitespace or end of line
                    let next_char = self.char_cache.get(i + 1).copied();
                    return next_char.map_or(true, |c| c.is_whitespace());
                }
                '\n' | '\r' => break, // End of line, no colon found
                _ => {}
            }
        }
        false
    }

    /// Check if there's an active mapping at the specified indentation level
    /// This method properly handles BlockEnd tokens by tracking mapping start/end pairs
    fn check_active_mapping_at_level(&self, _target_indent: usize) -> bool {
        let mut depth = 0;

        // Walk backwards through tokens to find the innermost unmatched block start.
        // Every BlockEnd increments depth; BlockMappingStart and BlockSequenceStart
        // decrement it (both open blocks that need a matching BlockEnd).
        // When depth == 0 we have found the block start that is still "open".
        for token in self.tokens.iter().rev() {
            match &token.token_type {
                TokenType::BlockMappingStart => {
                    if depth == 0 {
                        // The innermost open block is a mapping — active at this level.
                        return true;
                    }
                    depth -= 1;
                }
                TokenType::BlockSequenceStart => {
                    if depth == 0 {
                        // The innermost open block is a sequence, not a mapping.
                        return false;
                    }
                    depth -= 1;
                }
                TokenType::BlockEnd => {
                    depth += 1;
                }
                TokenType::StreamStart | TokenType::DocumentStart | TokenType::DocumentEnd => {
                    // Stop at document boundaries
                    break;
                }
                _ => {}
            }
        }

        false
    }
}

impl Scanner for BasicScanner {
    fn check_token(&self) -> bool {
        // For lazy scanning: check if we have cached tokens or can generate more
        self.token_index < self.tokens.len() || !self.done
    }

    fn peek_token(&self) -> Result<Option<&Token>> {
        // This is a bit tricky with lazy scanning since peek shouldn't mutate
        // For now, return cached token if available
        Ok(self.tokens.get(self.token_index))
    }

    fn get_token(&mut self) -> Result<Option<Token>> {
        // If we need more tokens and haven't finished, scan next token
        if self.token_index >= self.tokens.len() && !self.done {
            self.scan_next_token()?;
        }

        if self.token_index < self.tokens.len() {
            let token = self.tokens[self.token_index].clone();
            self.token_index += 1;
            Ok(Some(token))
        } else {
            Ok(None)
        }
    }

    fn reset(&mut self) {
        self.token_index = 0;
        self.position = Position::start();
        self.tokens.clear();
        self.done = false;
        self.current_char = self.input.chars().next();
        self.indent_stack = vec![0];
        self.current_indent = 0;
        self.flow_level = 0;
        self.detected_indent_style = None;
        self.indent_samples.clear();
        self.previous_indent_level = 0;
        self.current_char_index = 0;
        self.current_char = self.char_cache.first().copied();
    }

    fn position(&self) -> Position {
        self.position
    }

    fn input(&self) -> &str {
        &self.input
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drive the parser pipeline on `input` in a dedicated thread, returning
    /// `None` if it doesn't finish within `Duration::from_secs(2)`. Used by
    /// regression tests for parser hangs so a still-broken parser doesn't
    /// block the whole `cargo test` run.
    fn parse_with_timeout(input: &str) -> Option<Vec<crate::parser::Event>> {
        use crate::parser::{BasicParser, Parser as ParserTrait};
        use std::sync::mpsc;
        use std::thread;
        use std::time::Duration;

        let owned = input.to_string();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let mut p = BasicParser::new_eager(owned);
            let _ = p.take_scanning_error();
            let mut events = Vec::new();
            loop {
                match p.get_event() {
                    Ok(Some(ev)) => events.push(ev),
                    Ok(None) => break,
                    Err(_) => break,
                }
            }
            let _ = tx.send(events);
        });
        rx.recv_timeout(Duration::from_secs(2)).ok()
    }

    /// Regression: `---` directly followed by non-space text used to spin the
    /// scanner forever because the `-` match arm at line-start dispatched to
    /// `scan_document_start` (which correctly returned None) and then to
    /// `is_plain_scalar_start` (which returns false for `-`, so no consumption
    /// occurred — outer `while let` re-entered with the same char). Fix:
    /// fall through to `scan_plain_scalar` unconditionally when not a doc
    /// marker — the guard already ensures the char is non-whitespace.
    /// See yaml-test-suite tests 82AN / EXG3.
    #[test]
    fn three_dashes_directly_followed_by_text_does_not_hang() {
        let events = parse_with_timeout("---word1\nword2\n")
            .expect("parser hung — `---word1` should not produce an infinite loop");
        // We must produce at least one scalar whose value starts with `---`,
        // proving that the dashes were consumed as part of a plain scalar
        // (not interpreted as a document marker, which would consume them
        // separately).
        let starts_with_dashes = events.iter().any(|e| {
            matches!(&e.event_type,
                crate::parser::EventType::Scalar { value, .. } if value.starts_with("---")
            )
        });
        assert!(
            starts_with_dashes,
            "expected a plain scalar starting with `---`, got events: {events:?}"
        );
    }

    /// YAML 1.2 §7.3.3: `?`, `:`, and `-` may start a plain scalar provided
    /// the next character is non-space (and, in flow context, not a flow
    /// indicator). The previous `is_plain_scalar_start` unconditionally
    /// rejected those three characters, so plain scalars like `?foo`,
    /// `:foo`, `-foo` were reported as `Invalid character`.
    /// Tracked by yaml-test-suite 2EBW.
    #[test]
    fn question_mark_followed_by_text_starts_plain_scalar() {
        use crate::parser::{BasicParser, EventType, Parser as ParserTrait};
        let mut p = BasicParser::new_eager("?foo: bar\n".to_string());
        assert!(p.take_scanning_error().is_none());
        let mut keys = Vec::new();
        while let Ok(Some(ev)) = p.get_event() {
            if let EventType::Scalar { value, .. } = ev.event_type {
                keys.push(value);
            }
        }
        assert_eq!(keys, vec!["?foo", "bar"]);
    }

    #[test]
    fn colon_followed_by_text_starts_plain_scalar() {
        use crate::parser::{BasicParser, EventType, Parser as ParserTrait};
        let mut p = BasicParser::new_eager(":foo: bar\n".to_string());
        assert!(p.take_scanning_error().is_none());
        let mut keys = Vec::new();
        while let Ok(Some(ev)) = p.get_event() {
            if let EventType::Scalar { value, .. } = ev.event_type {
                keys.push(value);
            }
        }
        assert_eq!(keys, vec![":foo", "bar"]);
    }

    /// YAML 1.2: every started document must be closed with a DocumentEnd
    /// event before StreamEnd. The previous `TokenType::StreamEnd` handler
    /// only emitted `-DOC` for `DocumentContent` / `BlockNode` states —
    /// the `DocumentStart` state (entered after `---` and a single scalar
    /// like `"foo"`) was skipped, dropping the `-DOC` event. Affected by
    /// yaml-test-suite 27NA, 2G84/*, 2LFX and several others.
    #[test]
    fn explicit_doc_with_only_a_scalar_emits_doc_end_before_stream_end() {
        use crate::parser::{BasicParser, EventType, Parser as ParserTrait};
        let mut p = BasicParser::new_eager("---\n\"foo\"\n".to_string());
        assert!(p.take_scanning_error().is_none());
        let mut kinds = Vec::new();
        while let Ok(Some(ev)) = p.get_event() {
            kinds.push(match ev.event_type {
                EventType::StreamStart => "+STR",
                EventType::StreamEnd => "-STR",
                EventType::DocumentStart { .. } => "+DOC",
                EventType::DocumentEnd { .. } => "-DOC",
                EventType::Scalar { .. } => "=VAL",
                _ => "?",
            });
        }
        // Critical: -DOC must come before -STR.
        let doc_end_idx = kinds.iter().position(|s| *s == "-DOC");
        let str_end_idx = kinds.iter().position(|s| *s == "-STR");
        assert!(doc_end_idx.is_some(), "missing -DOC in event stream: {kinds:?}");
        assert!(doc_end_idx < str_end_idx, "expected -DOC before -STR, got {kinds:?}");
    }

    /// YAML 1.2 §5.7 hex / Unicode escapes in double-quoted strings.
    #[test]
    fn double_quoted_hex_escapes_decode_to_codepoint() {
        use crate::parser::{BasicParser, EventType, Parser as ParserTrait};
        for (input, expected) in [
            (r#""\x41""#, "A"),
            (r#""é""#, "é"),
            (r#""\U0001F600""#, "\u{1f600}"),
        ] {
            let mut p = BasicParser::new_eager(input.to_string());
            assert!(p.take_scanning_error().is_none(), "no scan error for {input}");
            let mut found = None;
            while let Ok(Some(ev)) = p.get_event() {
                if let EventType::Scalar { value, .. } = ev.event_type {
                    found = Some(value);
                    break;
                }
            }
            assert_eq!(found.as_deref(), Some(expected), "input {input}");
        }
    }

    #[test]
    fn truncated_hex_escape_is_a_scan_error() {
        use crate::parser::{BasicParser, Parser as ParserTrait};
        let mut p = BasicParser::new_eager(r#""\x4""#.to_string());
        assert!(p.take_scanning_error().is_some(), "truncated \\x escape must error");
    }

    /// YAML 1.2 §5.7: double-quoted strings have a strict allowlist of escape
    /// sequences. `\.` (and any other unknown escape) must be reported as a
    /// scan error. Tracked by yaml-test-suite 55WF.
    #[test]
    fn invalid_double_quoted_escape_is_a_scan_error() {
        use crate::parser::{BasicParser, Parser as ParserTrait};
        let mut p = BasicParser::new_eager("---\n\"\\.\"\n".to_string());
        let scan_err = p.take_scanning_error();
        let mut parse_err = false;
        if scan_err.is_none() {
            loop {
                match p.get_event() {
                    Ok(Some(_)) => continue,
                    Ok(None) => break,
                    Err(_) => {
                        parse_err = true;
                        break;
                    }
                }
            }
        }
        assert!(
            scan_err.is_some() || parse_err,
            "`\\.` is not a valid double-quoted escape and must error"
        );
    }

    /// YAML 1.2: a complex-key marker (`?`) is the first content after an
    /// explicit document start (`---`) — it should open an implicit block
    /// mapping. The previous parser handled `?` only in
    /// `ImplicitDocumentStart` / `DocumentContent` / already-in-mapping
    /// states and errored out for `DocumentStart`, breaking inputs like
    /// `--- !!set\n? Mark McGwire\n...`. Tracked by yaml-test-suite 2XXW.
    #[test]
    fn complex_key_directly_after_explicit_doc_start_opens_mapping() {
        use crate::parser::{BasicParser, EventType, Parser as ParserTrait};
        let mut p = BasicParser::new_eager(
            "--- !!set\n? Mark McGwire\n? Sammy Sosa\n".to_string(),
        );
        assert!(p.take_scanning_error().is_none());
        let mut saw_map_start = false;
        let mut saw_error = false;
        loop {
            match p.get_event() {
                Ok(Some(ev)) => {
                    if matches!(ev.event_type, EventType::MappingStart { .. }) {
                        saw_map_start = true;
                    }
                }
                Ok(None) => break,
                Err(_) => {
                    saw_error = true;
                    break;
                }
            }
        }
        assert!(!saw_error, "complex key after `--- !!set` must not error");
        assert!(saw_map_start, "expected a MappingStart event");
    }

    /// YAML 1.2 §6.9.2: anchor / alias names exclude only whitespace and
    /// the flow indicators `,[]{}`. Earlier implementations restricted
    /// `scan_identifier` to ASCII alphanumeric / `_` / `-`, which rejected
    /// valid unicode anchors like `&😁`. Tracked by yaml-test-suite 8XYN.
    #[test]
    fn anchor_name_may_contain_unicode_symbols() {
        use crate::parser::{BasicParser, EventType, Parser as ParserTrait};
        let mut p = BasicParser::new_eager("---\n- &😁 unicode anchor\n".to_string());
        assert!(p.take_scanning_error().is_none(), "unicode anchor must not error");
        let mut anchors = Vec::new();
        while let Ok(Some(ev)) = p.get_event() {
            if let EventType::Scalar { anchor: Some(a), .. } = ev.event_type {
                anchors.push(a);
            }
        }
        assert_eq!(anchors, vec!["😁"]);
    }

    /// YAML 1.2 §5.6 / RFC 3986 percent-encoding: tag suffixes may contain
    /// `%XX` percent-escaped characters, which must be URI-decoded when
    /// resolved. The scanner used to reject `%` in tag suffixes as
    /// "Invalid character", so e.g. `!e!tag%21 baz` failed before the
    /// resolver got a chance to decode it. Tracked by yaml-test-suite 6CK3.
    #[test]
    fn tag_suffix_with_percent_escape_resolves_to_decoded_uri() {
        use crate::parser::{BasicParser, EventType, Parser as ParserTrait};
        let mut p = BasicParser::new_eager(
            "%TAG !e! tag:example.com,2000:app/\n---\n- !e!tag%21 baz\n".to_string(),
        );
        assert!(p.take_scanning_error().is_none(), "tag percent-escapes must not error");
        let mut tags = Vec::new();
        while let Ok(Some(ev)) = p.get_event() {
            if let EventType::Scalar { tag: Some(t), .. } = ev.event_type {
                tags.push(t);
            }
        }
        assert_eq!(tags, vec!["tag:example.com,2000:app/tag!"]);
    }

    /// YAML 1.2 §6.8.4: "A YAML processor should ignore any directive it
    /// does not recognize." A `%FOO` reserved directive must NOT be treated
    /// as a scan error — the directive line is silently skipped and parsing
    /// continues. Tracked by yaml-test-suite test 2LFX.
    #[test]
    fn reserved_directive_is_ignored_not_an_error() {
        use crate::parser::{BasicParser, EventType, Parser as ParserTrait};
        let mut p = BasicParser::new_eager(
            "%FOO  bar baz # Should be ignored\n              # with a warning.\n---\n\"foo\"\n"
                .to_string(),
        );
        assert!(
            p.take_scanning_error().is_none(),
            "unknown directives must NOT produce a scan error"
        );
        let mut scalars = Vec::new();
        while let Ok(Some(ev)) = p.get_event() {
            if let EventType::Scalar { value, .. } = ev.event_type {
                scalars.push(value);
            }
        }
        assert_eq!(scalars, vec!["foo"]);
    }

    /// Spec requires the two physical lines of `---word1\nword2` to fold into
    /// a single plain scalar `"---word1 word2"`. Tracked by yaml-test-suite 82AN.
    #[test]
    fn three_dashes_followed_by_text_folds_continuation_line() {
        let events = parse_with_timeout("---word1\nword2\n")
            .expect("parser hung");
        let scalars: Vec<&str> = events
            .iter()
            .filter_map(|e| match &e.event_type {
                crate::parser::EventType::Scalar { value, .. } => Some(value.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(scalars, vec!["---word1 word2"]);
    }

    /// Regression: tab between block-entry marker and a `-N` value used to
    /// hang the scanner via the same `-` match arm. See yaml-test-suite
    /// Y79Y/010.
    #[test]
    fn dash_tab_negative_number_does_not_hang() {
        let events = parse_with_timeout("-\t-1\n")
            .expect("parser hung — `-\\t-1` should not produce an infinite loop");
        assert!(!events.is_empty(), "expected event stream, got none");
    }

    #[test]
    fn test_basic_tokenization() {
        let mut scanner = BasicScanner::new("42".to_string());

        assert!(scanner.check_token());

        // StreamStart
        let token = scanner.get_token().unwrap().unwrap();
        assert!(matches!(token.token_type, TokenType::StreamStart));

        // Number
        let token = scanner.get_token().unwrap().unwrap();
        if let TokenType::Scalar(value, _) = token.token_type {
            assert_eq!(value, "42");
        } else {
            panic!("Expected scalar token");
        }

        // StreamEnd
        let token = scanner.get_token().unwrap().unwrap();
        assert!(matches!(token.token_type, TokenType::StreamEnd));
    }

    #[test]
    fn test_flow_sequence() {
        let mut scanner = BasicScanner::new("[1, 2, 3]".to_string());

        // StreamStart
        scanner.get_token().unwrap();

        // [
        let token = scanner.get_token().unwrap().unwrap();
        assert!(matches!(token.token_type, TokenType::FlowSequenceStart));

        // 1
        let token = scanner.get_token().unwrap().unwrap();
        if let TokenType::Scalar(value, _) = token.token_type {
            assert_eq!(value, "1");
        }

        // ,
        let token = scanner.get_token().unwrap().unwrap();
        assert!(matches!(token.token_type, TokenType::FlowEntry));
    }

    #[test]
    fn test_quoted_strings() {
        let mut scanner = BasicScanner::new(r#""hello world""#.to_string());

        // StreamStart
        scanner.get_token().unwrap();

        // Quoted string
        let token = scanner.get_token().unwrap().unwrap();
        if let TokenType::Scalar(value, _) = token.token_type {
            assert_eq!(value, "hello world");
        } else {
            panic!("Expected scalar token");
        }
    }

    #[test]
    fn test_comment_handling() {
        let input = r"
# Full line comment
key: value  # End of line comment
# Another comment
data: test
";
        let mut scanner = BasicScanner::new(input.to_string());

        let mut tokens = Vec::new();
        while let Ok(Some(token)) = scanner.get_token() {
            tokens.push(token);
        }

        // Should only contain YAML structure tokens, no comment tokens
        let scalar_values: Vec<String> = tokens
            .iter()
            .filter_map(|t| match &t.token_type {
                TokenType::Scalar(s, _) => Some(s.clone()),
                _ => None,
            })
            .collect();

        assert_eq!(scalar_values, vec!["key", "value", "data", "test"]);

        // Should not contain any comment tokens
        assert!(!tokens
            .iter()
            .any(|t| matches!(t.token_type, TokenType::Comment(_))));
    }

    #[test]
    fn test_hash_in_strings() {
        let input = r#"
string1: "This has a # character"
string2: 'Also has # character'
normal: value # This is a comment
"#;
        let mut scanner = BasicScanner::new(input.to_string());

        let mut scalar_values = Vec::new();
        while let Ok(Some(token)) = scanner.get_token() {
            if let TokenType::Scalar(value, _) = token.token_type {
                scalar_values.push(value);
            }
        }

        assert!(scalar_values.contains(&"This has a # character".to_string()));
        assert!(scalar_values.contains(&"Also has # character".to_string()));
        assert!(scalar_values.contains(&"value".to_string()));
        assert!(!scalar_values
            .iter()
            .any(|s| s.contains("This is a comment")));
    }

    #[test]
    fn test_escape_sequences() {
        // YAML 1.2 §5.7 double-quoted escape sequences. Single-quoted strings
        // have NO backslash escapes — `''` is the only escape — so this set
        // is restricted to the double-quoted cases.
        let test_cases = vec![
            (r#""Line 1\nLine 2""#, "Line 1\nLine 2"),
            (r#""Col1\tCol2""#, "Col1\tCol2"),
            (r#""First\rSecond""#, "First\rSecond"),
            (r#""Path\\to\\file""#, "Path\\to\\file"),
            (r#""He said \"Hello\"""#, "He said \"Hello\""),
        ];

        for (input, expected) in test_cases {
            let mut scanner = BasicScanner::new(input.to_string());
            scanner.get_token().unwrap(); // Skip StreamStart

            if let Ok(Some(token)) = scanner.get_token() {
                if let TokenType::Scalar(value, _) = token.token_type {
                    assert_eq!(value, expected, "Failed for input: {}", input);
                } else {
                    panic!("Expected scalar token for input: {}", input);
                }
            } else {
                panic!("Failed to get token for input: {}", input);
            }
        }
    }

    #[test]
    fn test_extended_yaml_escapes() {
        // Test additional YAML escape sequences
        let test_cases = vec![
            (r#""\0""#, "\0"),   // null character
            (r#""\a""#, "\x07"), // bell
            (r#""\b""#, "\x08"), // backspace
            (r#""\f""#, "\x0C"), // form feed
            (r#""\v""#, "\x0B"), // vertical tab
            (r#""\e""#, "\x1B"), // escape
            (r#""\ ""#, " "),    // literal space
            (r#""\/""#, "/"),    // literal forward slash
        ];

        for (input, expected) in test_cases {
            let mut scanner = BasicScanner::new(input.to_string());
            scanner.get_token().unwrap(); // Skip StreamStart

            if let Ok(Some(token)) = scanner.get_token() {
                if let TokenType::Scalar(value, _) = token.token_type {
                    assert_eq!(value, expected, "Failed for input: {}", input);
                } else {
                    panic!("Expected scalar token for input: {}", input);
                }
            } else {
                panic!("Failed to get token for input: {}", input);
            }
        }
    }

    #[test]
    fn test_unknown_escape_sequences() {
        // YAML 1.2 §5.7: unknown double-quoted escapes are scan errors, not
        // preserved literals. (Earlier versions of this scanner kept the
        // backslash + char verbatim — see commit history.)
        for input in [r#""\z""#, r#""\q""#, r#""\8""#] {
            let mut scanner = BasicScanner::new(input.to_string());
            scanner.get_token().unwrap(); // StreamStart
            assert!(
                scanner.get_token().is_err(),
                "expected scan error for invalid escape in {input}"
            );
        }
    }
}
