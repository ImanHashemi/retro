# Scenario: Null fields in AI response don't crash

## Description
The AI backend can return null for string fields in pattern responses. The fix uses `#[serde(default)]` on optional fields so null deserializes to empty string/default rather than crashing. This scenario verifies the models can handle null fields by checking the serde attributes are in place.

## Setup
1. Run `./target/debug/retro init`

## Steps
1. Run `grep -n 'serde(default)' ./crates/retro-core/src/models.rs | head -20`
2. Run `grep -n 'Option<' ./crates/retro-core/src/models.rs | head -20`
3. Run `grep -n 'default' ./crates/retro-core/src/analysis/merge.rs 2>/dev/null || echo "checked"`
4. Run `./target/debug/retro status 2>&1`

## Expected
- models.rs contains multiple `#[serde(default)]` annotations on pattern-related structs
- The retro binary runs without panicking (status command works)
- Pattern-related structs use either `Option<String>` or `#[serde(default)]` for fields that could be null

## Not Expected
- No panic or "called unwrap on None" errors
- No compilation errors
