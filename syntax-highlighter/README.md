# Briev Language - VSCode Extension

Syntax highlighting, folding, bracket colorization, and intelligent editing
for the whole Briev file family:

| Extension | Language | Grammar |
|-----------|----------|---------|
| `.bv` | Briev | `source.briev` |
| `.rbv` | Rendered Briev (web) | `source.rbv` (+ HTML/view regions) |
| `.ebv` | Electronics Briev | wrapper over `source.briev` |
| `.sbv` | Silicon Briev | wrapper over `source.briev` |
| `.abv` | Accelerator Briev (GPU) | wrapper over `source.briev` |
| `.bld` | BILLD — intermediate low-level | `source.bld` (nests `source.bad` in `bad { }`) |
| `.bad` | Briev Assembly Dialect | `source.bad` (nests into `.bv` `bad fn` / `bootstrap bad` bodies) |
| `.dbv`, `.dbvl` | Data Briev | `source.dbriev` |
| `.beast` | BEAST snapshots (S-expressions) | `source.beast` |

Nested dialects: `.bv` files color `bad name(…) { … }` and
`bootstrap bad name(…) { … }` bodies with the real `.bad` rules;
`.bld` files color `bad { … }` blocks the same way.

## Features

- **Syntax Highlighting**: Keywords, types, operators, strings, numbers,
  comments, quantity literals (`330Ohm`, `3.3Volt`, `4k7`), compound
  assigns (`+=` … `>>=`), hash-words (`#Lh`, `#System`, `#Link<name>`,
  `###`), retired spellings (`#Int`) marked as errors
- **Bracket Colorization**: Matching brackets highlighted in different colors
- **Auto-closing**: Automatically closes `{}`, `[]`, `()`, and `""`
- **Auto-indentation**: Smart indentation for Briev code blocks
- **Themes**: Dark and Light themes included

## Installation

### Option 1: Link Locally (Recommended for Development)

```bash
# Create symlink to extensions folder
ln -s "$(pwd)" ~/.vscode/extensions/briev-language
# Or for VScodium:
ln -s "$(pwd)" ~/.config/VSCodium/User/extensions/briev-language
```

### Option 2: Package as .vsix

```bash
npm install -g @vscode/vsce
vsce package
code --install-extension briev-language-0.2.0.vsix
```

## Color Scheme

| Element | Color | Style |
|---------|-------|-------|
| `txn` | Purple | Bold |
| `node` | Yellow | Bold |
| `term`/`escape` | Gold | Bold |
| `defn`/`let` | Blue | Normal |
| `state`/`plain` assertions | Purple | Bold |
| Types | Cyan | Normal |
| Custom Types | Green | Normal |
| `true`/`false` | Blue | Bold |
| Strings | Orange | Normal |
| Comments | Green | Italic |
| `&` ownership | Purple | Bold |
| `@` prior state | Yellow | Italic |
| Retired `#Int`-style hashwords | Red | Strikethrough |

## Keyboard Shortcuts

| Action | Shortcut |
|--------|----------|
| Fold all | `Ctrl+Shift+[` |
| Unfold all | `Ctrl+Shift+]` |
| Fold region | `Ctrl+Shift+Ctrl+[` |
| Format document | `Alt+Shift+F` |

## Settings

Enable bracket colorization:
```json
{
  "editor.bracketPairColorization.enabled": true,
  "editor.guides.bracketPairs": true
}
```

## Development

To modify a grammar:
1. Edit the grammar under `syntaxes/`
2. Validate the whole set: `node syntax-highlighter/validate_grammars.mjs`
   (JSON parse, regex compile, contribution consistency, corpus smoke-matches)
3. Reload VSCode window

To modify themes:
1. Edit `themes/briev-dark.json` or `themes/briev-light.json`
2. Reload window

## Extension Structure

```
briev-language/
├── package.json                 # Extension manifest (languages + grammars)
├── language-configuration.json  # Brackets, // comments, indentation
├── language-configuration-bld.json  # .bld-specific editor config
├── settings.json                # Default settings
├── syntaxes/
│   ├── briev.tmLanguage.json    # Core Briev (nests #bad-body)
│   ├── rbv.tmLanguage.json      # Rendered Briev (view/style regions)
│   ├── ebv.tmLanguage.json      # wrapper over source.briev
│   ├── sbv.tmLanguage.json      # wrapper over source.briev
│   ├── abv.tmLanguage.json      # wrapper over source.briev
│   ├── bld.tmLanguage.json      # BILLD (nests source.bad)
│   ├── bad.tmLanguage.json      # Briev Assembly Dialect
│   ├── dbriev.tmLanguage.json   # Data Briev (schema/table format)
│   └── beast.tmLanguage.json    # BEAST S-expressions
├── images/                      # Per-language icons
├── themes/
│   ├── briev-dark.json          # Dark theme
│   └── briev-light.json         # Light theme
├── validate_grammars.mjs        # Grammar validation gate
└── README.md
```
