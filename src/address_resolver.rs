// ── Shared Address Resolver ────────────────────────────────────────────
//
// 2026-07-15: Resolves named device/entity identifiers to numeric MMIO
// addresses. Used by both the interpreter (AddressOf# evaluation) and the
// LLVM backend (AddressOf# codegen).
//
// 2026-08-03 (Phase 2, plan docs/plans/2026-08-03-data-briev-config-and-
// board-hardware-map.md §5.2): the board's `addresses.dbvl` is now the
// primary source. Resolution order:
//   1. active board's `lib/boards/<board>/addresses.dbvl` (ConfigDb-backed)
//   2. config/address-map.dbvl (deprecated alias)
//   3. hardcoded table — with a warning (an unowned default must say so)
//   4. default MMIO region base (0xFE000000)
//
// The active board is selected via `import "target"` (the `--board`
// mechanism in `ImportResolver`); it defaults to "stm32f407" and is stored
// in a thread-local so both backends and the interpreter agree by
// construction — they all call this one function.

use crate::dbriev::config_db::ConfigDb;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

thread_local! {
    /// Active board map: normalized key (uppercase) → address.
    /// Set once per compilation by `set_active_board` (via `import "target"`).
    static ACTIVE_BOARD: RefCell<Option<HashMap<String, u64>>> = const { RefCell::new(None) };
    /// 2026-09-06 (ISR plan): active board's named vector table — normalized
    /// key (uppercase) → vector slot index. Loaded alongside the address map
    /// by `set_active_board` from `lib/boards/<board>/interrupts.dbvl`
    /// (missing file = no named vectors; only literal `@ n` bindings work).
    static ACTIVE_ISR_TABLE: RefCell<Option<HashMap<String, u64>>> = const { RefCell::new(None) };
}

/// Default board name, matching `ImportResolver::resolve_target_import`.
const DEFAULT_BOARD: &str = "stm32f407";

/// 2026-08-03: Set the active board's address table from its
/// `lib/boards/<board>/addresses.dbvl`. Non-fatal: a missing board file
/// leaves the resolver on the config/hardcoded fallbacks.
pub fn set_active_board(board: &str) {
    let Some(path) = find_board_path(board, "addresses.dbvl") else {
        return;
    };
    let Ok(db) = ConfigDb::from_file(&path, false) else {
        return;
    };
    let mut map = HashMap::new();
    for key in db.keys() {
        if let Some(s) = db.field_string(&key, 0) {
            if let Some(addr) = radix_parse_hex(s) {
                map.insert(key.to_uppercase(), addr);
            }
        }
    }
    ACTIVE_BOARD.with(|slot| *slot.borrow_mut() = Some(map));
    // 2026-09-06 (ISR plan): load the named vector table alongside —
    // `isr handler @ TIM2` resolves through the same board directory
    // (interrupts.dbvl: `TIM2: 28;`). Missing file is non-fatal: only
    // literal vector bindings are available on this board.
    if let Some(isr_path) = find_board_path(board, "interrupts.dbvl") {
        if let Ok(isr_db) = ConfigDb::from_file(&isr_path, false) {
            let mut vectors = HashMap::new();
            for key in isr_db.keys() {
                // Slot values accept Int (decimal `TIM2: 28;`) or String
                // (hex `TIM2: 0x1C;`) — the parser stores by literal shape.
                let vec = isr_db
                    .field_int(&key, 0)
                    .map(|v| v as u64)
                    .or_else(|| isr_db.field_string(&key, 0).and_then(radix_parse_hex));
                if let Some(vec) = vec {
                    vectors.insert(key.to_uppercase(), vec);
                }
            }
            ACTIVE_ISR_TABLE.with(|slot| *slot.borrow_mut() = Some(vectors));
        }
    }
}

/// 2026-09-06 (ISR plan): resolve a NAMED vector through the active
/// board's interrupts.dbvl. None = the name has no row (the caller
/// errors with the board file and the fix).
pub fn resolve_isr_vector(name: &str) -> Option<u64> {
    ACTIVE_ISR_TABLE.with(|slot| {
        slot.borrow()
            .as_ref()
            .and_then(|m| m.get(&name.to_uppercase()).copied())
    })
}

/// 2026-07-15: Resolve a named address to its numeric value.
///
/// Tries, in order:
/// 1. Active board's addresses.dbvl (if set)
/// 2. config/address-map.dbvl (if present)
/// 3. Hardcoded well-known device names (with a warning)
/// 4. Default MMIO region base (0xFE000000)
pub fn resolve_address(id: &str) -> u64 {
    // Board map first (case-insensitive, like the config path below).
    let upper = id.to_uppercase();
    if let Some(addr) =
        ACTIVE_BOARD.with(|slot| slot.borrow().as_ref().and_then(|m| m.get(&upper).copied()))
    {
        return addr;
    }
    // Config file second (deprecated alias).
    if let Some(addr) = resolve_from_config(id) {
        return addr;
    }
    // Hardcoded table — warn: an unowned default must say so.
    if let Some(addr) = resolve_from_hardcoded(id) {
        eprintln!(
            "AddressOf#: warning — '{}' resolved from the hardcoded fallback; \
             add it to the active board's addresses.dbvl to own it.",
            id
        );
        return addr;
    }
    // Default MMIO region base.
    0xFE000000
}

/// 2026-07-15: Read config/address-map.dbvl and look up the id.
/// 2026-08-03 (Phase 3): migrated from address-map.dbvl to the .dbvl
/// line-table form; the lookup is case-insensitive like the board map.
fn resolve_from_config(id: &str) -> Option<u64> {
    let config_path = find_config_path()?;
    let db = ConfigDb::from_file(&config_path, false).ok()?;
    let s = db.field_string(&id.to_uppercase(), 0)?;
    radix_parse_hex(s)
}

/// 2026-07-15: Locate config/address-map.dbvl relative to the project root
/// or the compiler binary.
fn find_config_path() -> Option<PathBuf> {
    // Try relative to CWD (development)
    let candidates = [
        PathBuf::from("config/address-map.dbvl"),
        PathBuf::from("config/address-map.dbv"),
    ];
    for c in &candidates {
        if c.exists() {
            return Some(c.clone());
        }
    }
    // Try relative to executable
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            for c in &candidates {
                let path = parent.join(c);
                if path.exists() {
                    return Some(path);
                }
            }
        }
    }
    None
}

/// 2026-08-03: Locate `lib/boards/<board>/<file>` relative to CWD, the
/// project lib/, or the executable (dev layout: target/ -> ../../lib/).
fn find_board_path(board: &str, file: &str) -> Option<PathBuf> {
    let rel = format!("{}/{}", board, file);
    for candidate in [
        PathBuf::from("lib").join("boards").join(&rel),
        PathBuf::from("boards").join(&rel),
    ] {
        if candidate.exists() {
            return Some(candidate);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            let dev = parent.join("../../lib/boards").join(&rel);
            if dev.exists() {
                return Some(dev);
            }
        }
    }
    None
}

/// Parse a `0x`-prefixed (or bare-hex) string into a u64.
fn radix_parse_hex(s: &str) -> Option<u64> {
    let clean = s.trim_start_matches("0x").trim_start_matches("0X");
    u64::from_str_radix(clean, 16).ok()
}

/// Hardcoded well-known device address table.
/// 2026-07-15: Mirrors config/address-map.dbvl for fallback when the
/// board map and config file are not available. 2026-08-03: reaches here
/// only after the board map and config both miss — and emits a warning.
fn resolve_from_hardcoded(id: &str) -> Option<u64> {
    match id.to_lowercase().as_str() {
        "uart" | "uart0" | "/dev/ttys0" | "/dev/ttyama0" => Some(0xFFE01000),
        "gpio" | "gpio0" | "/dev/gpiochip0" => Some(0xFE001000),
        "timer" | "timer0" | "/dev/timer0" => Some(0xFE002000),
        "spi" | "spi0" | "/dev/spidev0.0" => Some(0xFE003000),
        "i2c" | "i2c0" | "/dev/i2c-0" => Some(0xFE004000),
        "dma" | "dma0" | "/dev/dma" => Some(0xFE005000),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clear_active_board() {
        ACTIVE_BOARD.with(|slot| *slot.borrow_mut() = None);
    }

    #[test]
    fn test_resolve_address_known() {
        clear_active_board();
        let addr = resolve_address("uart");
        assert_eq!(addr, 0xFFE01000);
    }

    #[test]
    fn test_resolve_address_known_case_insensitive() {
        clear_active_board();
        let addr = resolve_address("UART");
        assert_eq!(addr, 0xFFE01000);
    }

    #[test]
    fn test_resolve_address_unknown_defaults() {
        clear_active_board();
        let addr = resolve_address("nonexistent_device");
        assert_eq!(addr, 0xFE000000);
    }

    #[test]
    fn test_resolve_address_from_config_file() {
        // This test passes if config/address-map.dbvl is present and
        // contains "uart" → 0xFFE01000. If the file is absent, fallback
        // kicks in and still returns the right value.
        clear_active_board();
        let addr = resolve_address("gpio0");
        assert_eq!(addr, 0xFE001000);
    }

    #[test]
    fn test_resolve_from_hardcoded_device_paths() {
        clear_active_board();
        let addr = resolve_address("/dev/ttyama0");
        assert_eq!(addr, 0xFFE01000);
    }

    #[test]
    fn test_board_map_beats_config_and_hardcoded() {
        // The stm32f407 board map owns UART1 at its real address; the
        // generic config/hardcoded "uart" name is untouched.
        set_active_board("stm32f407");
        let addr = resolve_address("uart1");
        assert_eq!(addr, 0x40011000);
        assert_eq!(resolve_address("UART1"), 0x40011000);
        assert_eq!(resolve_address("gpioa"), 0x40020000);
        // Config/hardcoded names still resolve through their own path.
        assert_eq!(resolve_address("uart"), 0xFFE01000);
        clear_active_board();
    }

    #[test]
    fn test_board_map_missing_is_non_fatal() {
        clear_active_board();
        set_active_board("no-such-board");
        // Falls back cleanly to config/hardcoded.
        assert_eq!(resolve_address("uart"), 0xFFE01000);
    }
}

#[cfg(test)]
mod isr_tests {
    #[test]
    fn board_isr_table_loads_and_resolves() {
        super::set_active_board("stm32f407");
        assert_eq!(
            super::resolve_isr_vector("TIM2"),
            Some(28),
            "TIM2 must resolve through lib/boards/stm32f407/interrupts.dbvl"
        );
        assert_eq!(
            super::resolve_isr_vector("tim2"),
            Some(28),
            "case-insensitive lookup"
        );
        assert_eq!(super::resolve_isr_vector("NOPE"), None);
    }
}
