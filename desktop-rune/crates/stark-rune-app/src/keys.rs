//! Web-parity Enter activation for ordinary controls.
//!
//! Rune follows AppKit: Space presses the focused control, while Return goes
//! to a link, a highlighted/list row, or the window's `default_button`. A web
//! `<button>` presses on both Enter and Space, so every Stark button uses
//! these helpers instead of bare [`rune::El::on_press`]. `on_key` runs before
//! focus navigation and the built-in press, and returning `true` consumes the
//! key, so a button inside a list group still fires exactly once.

use rune::{El, Key, KeyEvent, Mods, Tx};

/// Adds Enter activation beside a button's ordinary press handler.
pub trait EnterPress: Sized {
    /// The web `<button>` behavior for an `on_press` handler.
    fn on_press_enter<S: 'static>(self, f: impl Fn(&mut S) + 'static) -> Self;
    /// The same for a handler that needs the commit transaction.
    fn on_press_tx_enter<S: 'static>(self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self;
}

fn plain_enter(key: &KeyEvent) -> bool {
    key.key == Key::Enter && key.mods == Mods::default()
}

impl EnterPress for El {
    fn on_press_enter<S: 'static>(self, f: impl Fn(&mut S) + 'static) -> Self {
        let enter = std::rc::Rc::new(f);
        let press = enter.clone();
        self.on_press(move |s: &mut S| press(s))
            .on_key(move |s: &mut S, key, _tx| {
                if plain_enter(key) {
                    enter(s);
                    true
                } else {
                    false
                }
            })
    }

    fn on_press_tx_enter<S: 'static>(self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        let enter = std::rc::Rc::new(f);
        let press = enter.clone();
        self.on_press_tx(move |s: &mut S, tx: &mut Tx| press(s, tx))
            .on_key(move |s: &mut S, key, tx| {
                if plain_enter(key) {
                    enter(s, tx);
                    true
                } else {
                    false
                }
            })
    }
}

#[cfg(test)]
mod tests {
    use super::EnterPress;
    use rune::{col, el, Env, Key, KeyEvent, Mods, Runtime};

    #[derive(Default)]
    struct Hits {
        presses: u32,
    }

    fn view(_s: &Hits, _cx: &mut rune::Cx<Hits>) -> rune::El {
        col().w_full().h_full().child(
            el().key("only")
                .size(80.0, 32.0)
                .role("button")
                .focusable(true)
                .label("Only")
                .on_press_enter(|s: &mut Hits| s.presses += 1),
        )
    }

    fn press(key: Key, hits: &mut u32) {
        let mut rt = Runtime::new(Hits::default(), view, (200.0, 120.0), Env::new(true, 1.0));
        rt.set_full_keyboard_access(true);
        rt.frame(0.0);
        rt.key(KeyEvent {
            key: Key::Tab,
            mods: Mods::default(),
        });
        rt.frame(0.0);
        rt.key(KeyEvent {
            key,
            mods: Mods::default(),
        });
        rt.frame(0.0);
        *hits = rt.state.presses;
    }

    #[test]
    fn enter_and_space_both_press_a_button_once() {
        let mut enter = 0;
        press(Key::Enter, &mut enter);
        assert_eq!(enter, 1, "Enter presses the focused button once");
        let mut space = 0;
        press(Key::Text(" ".to_string()), &mut space);
        assert_eq!(space, 1, "Space keeps Rune's native activation");
    }
}
