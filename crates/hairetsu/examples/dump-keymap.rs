//! Print the emitted XKB keymap, so it can be compiled by a real XKB
//! implementation as a differential check on the wire format.
fn main() {
    print!("{}", hairetsu::Keymap::us().as_text());
}
