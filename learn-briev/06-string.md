# String Manipulation

> **Runnable companion:** `learn-briev/examples/06-string.bv` compiles, runs, and its output is pinned in CI (`tests/learn_briev_examples.rs`) — the machine-checked face of this chapter.


Briev provides comprehensive string operations, all implemented natively (no FFI).

## 0. String and byte literals

Three literal forms (SPEC §16.1/16.2):

```briev
let s: String = "escaped\n";     // escapes ARE interpreted: \n \t \\ \" \xHH
let r: String = #r"raw \ text";  // raw string: escapes NOT interpreted
let b: Blob   = #b"\x89PNG\r\n"; // byte literal: raw bytes (exact \xHH content)
```

- `#r"..."` is a raw string — backslashes are kept literally (useful for regex
  and Windows paths).
- `#b"..."` is a Blob byte literal — `\xHH` yields the exact byte, and the
  result is typed `Blob`, not `String`. `x::Len` on a Blob value is the byte
  length (O(1), reads the `[len]` header). `Blob` is the `[len][bytes]` byte
  buffer — raw bytes with no encoding interpretation; `String` is the same
  buffer interpreted as UTF-8. `blob as String` is an unchecked lens.

```briev
let bytes: Blob = #b"\x00\xff";
let n: Int = bytes::Length;  // 2
```

## 1. Basic Operations

```briev
let s: String = "Hello, World!";

// Length (method syntax)
let len = s::Length;  // 13

// Concatenation
let combined = "Hello" + "World";  // "HelloWorld"

// Substring
let sub = s.substr(0, 5);  // "Hello"

// Character at index
let c = s.char_at(0);  // "H"
```

## 2. Search Operations

```briev
let s: String = "Hello, World!";

// Contains
[string.contains(s, "World")] {
    println("Found World");
};

// Find index
let idx = string.find(s, "World");  // 7
[idx >= 0] {
    println("Found at index: " + String(idx));
};

// Starts/Ends with
[string.starts_with(s, "Hello")] {
    println("Starts with Hello");
};

[string.ends_with(s, "!")] {
    println("Ends with !");
};
```

## 3. Case Conversion

```briev
let s: String = "Hello, World!";

let lower = string.to_lower(s);  // "hello, world!"
let upper = string.to_upper(s);  // "HELLO, WORLD!"
let capitalized = string.capitalize(s);  // "Hello, world!"
let title = string.title_case(s);  // "Hello, World!"
```

## 4. Trimming

```briev
let s: String = "  Hello  ";

let trimmed = string.trim(s);  // "Hello"
let left_trimmed = string.trim_left(s);  // "Hello  "
let right_trimmed = string.trim_right(s);  // "  Hello"
```

## 5. Split and Join

```briev
let s: String = "apple,banana,cherry";

// Split
let parts = string.split(s, ",");  // ["apple", "banana", "cherry"]

// Join
let joined = string.join(parts, " | ");  // "apple | banana | cherry"

// Lines
let text: String = "Line 1\nLine 2\nLine 3";
let lines = string.lines(text);  // ["Line 1", "Line 2", "Line 3"]
```

## 6. Replace

```briev
let s: String = "Hello, World!";

// Replace first occurrence
let replaced = string.replace(s, "World", "Briev");  // "Hello, Briev!"

// Replace all (FFI - will be native soon)
// let all = string.replace_all(s, "l", "L");  // "HeLLo, WorLd!"
```

## 7. Padding

```briev
let s: String = "42";

let left_padded = string.pad_left(s, 5);  // "   42"
let right_padded = string.pad_right(s, 5);  // "42   "
let centered = string.pad_center(s, 5);  // " 42  "

// With custom character
let zero_padded = string.pad_left_with(s, 5, '0');  // "00042"
```

## 8. Character Classification

```briev
let s: String = "Hello123";

// Check all characters
[string.is_alpha(s)] {
    println("All alphabetic");
};

[string.is_numeric(s)] {
    println("All numeric");
};

[string.is_whitespace(s)] {
    println("All whitespace");
};

[string.is_alphanumeric(s)] {
    println("All alphanumeric");
};

// Count specific characters
let spaces = string.count_char(s, 'l');  // 2
```

## 9. String Building (Efficient)

For building strings incrementally, use StringBuilder (O(1) append):

```briev
let sb = new_builder();
sb = sb.append_str("Hello");
sb = sb.append_char(',');
sb = sb.append_char(' ');
sb = sb.append_str("World");
sb = sb.append_char('!');
let result = sb.to_string();  // "Hello, World!"
```

## 10. Complete Example: Text Analyzer

```briev
// text_analyzer.bv

struct TextStats {
    char_count: Int,
    word_count: Int,
    line_count: Int,
    space_count: Int,
    upper_count: Int,
    lower_count: Int
};

defn analyze_text(text: String) -> TextStats {
    let stats = TextStats {
        char_count: 0,
        word_count: 0,
        line_count: 0,
        space_count: 0,
        upper_count: 0,
        lower_count: 0
    };
    
    let words = text.split(" ");
    let lines = text.split("\n");
    
    stats.char_count = CharCount#(text);
    stats.word_count = CharCount#(words);
    stats.line_count = CharCount#(lines);
    
    // Count spaces
    let i: Int = 0;
    [i < text::Len] {
        let c = text.char_at(i);
        [c == ' '] {
            &stats.space_count = stats.space_count + 1;
        };
        [c.is_upper()] {
            &stats.upper_count = stats.upper_count + 1;
        };
        [c.is_lower()] {
            &stats.lower_count = stats.lower_count + 1;
        };
        i = i + 1;
    };
    
    term stats;
};

defn reverse_words(text: String) -> String {
    let words = text.split(" ");
    let reversed: List<String> = [];
    
    let i: Int = words::Len - 1;
    [i >= 0] {
        reversed = reversed.append(words[i]);
        i = i - 1;
    };
    
    term string.join(reversed, " ");
};

defn is_palindrome(text: String) -> Bool {
    let cleaned = string.to_lower(string.trim(text));
    let reversed = string.reverse(cleaned);
    term cleaned == reversed;
};

node run [!done][done == true] {
    let text: String = "Hello World";
    let stats = analyze_text(text);
    
    println("Characters: " + String(stats.char_count));
    println("Words: " + String(stats.word_count));
    println("Lines: " + String(stats.line_count));
    println("Uppercase: " + String(stats.upper_count));
    println("Lowercase: " + String(stats.lower_count));
    
    done = true;
    term;
};
```

## Exercises

1. Implement a URL parser that extracts protocol, domain, and path
2. Create a simple Markdown to HTML converter
3. Build a CSV parser that handles quoted fields
4. Implement a basic search-and-replace with regex-like patterns

---

*Next: [07-ffi.md](07-ffi.md) - Foreign Function Interface*
