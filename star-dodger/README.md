# Star Dodger

A small arcade dodger in one self-contained HTML file — HTML5 canvas + vanilla
JavaScript, no libraries, no build step, no network calls.

## Run it

1. Download or clone this folder.
2. **Double-click `index.html`** (or drag it into any modern browser: Chrome,
   Edge, Firefox, Safari).

That's it — the game runs straight from `file://`. A local web server is
optional; if you prefer one, `python -m http.server` inside this folder and open
`http://localhost:8000/` works exactly the same.

## Controls

| Action | Keys |
| --- | --- |
| Start game | `Space` or `Enter` |
| Move left | `←` or `A` |
| Move right | `→` or `D` |
| Restart after game over | `R`, `Space`, or `Enter` |

## How to play

- Your ship sits at the bottom of the screen; asteroids fall from the top.
- **Score** grows with survival time (10 points per second).
- **Speed** ramps up gradually: asteroids fall faster and spawn more often the
  longer you last (up to 3.2×).
- Touching an asteroid ends the run and shows your final score plus your best
  score, which is remembered in `localStorage` when the browser allows it.

## Notes

- The canvas is responsive and scales for high-DPI screens; resizing the window
  mid-run is safe.
- Everything lives in `index.html`: markup, styles, and game code.
