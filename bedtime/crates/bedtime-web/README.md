# bedtime-web

A web browser panel for workbench apps, on macOS. The system's web view
(WKWebView, through [wry](https://github.com/tauri-apps/wry)) shows the
pages, so nothing is bundled and the system keeps the engine up to date.

```rust,ignore
app.add_plugins(WebPlugin);
```

- **Opening.** `web.show` ("Web Browser", in the View menu) focuses the web
  panel in the active layout, else opens a blank one. `WebPanel { url }`
  opens a page; the address is saved with the panel.
- **From elsewhere.** `open_url(world, url)` shows an address in the web
  panel of the active layout (sent there by a `Navigate` component the panel
  takes when it's drawn), else in a new one. `web_address` says whether some
  text is plainly a web address (`https://…`, `www.…`, `localhost:…`; not a
  bare `bevy.org`, which looks like a file name), and `with_scheme` makes
  anything typed into one. An app can put these behind bedtime-ui's
  `ExternalOpener`, so links clicked anywhere open here.
- **Controls.** Under the page: back, forward, reload and the address. The
  field shows a link's target while the pointer is over it. The page's title
  is the tab's. A link that asks for a new window opens in the panel.
- **A native view.** The web view isn't part of the app's picture: it is laid
  over the page's rectangle and moved with it, above everything the app
  draws, and takes its own input. The controls are under the page so that
  rectangle stays clear of the panel's rounded bottom corners. Clicking the
  page focuses its panel; focusing another panel, or the address field, takes
  the keyboard back from the web view.
- **Drawn pages** (`DrawWebPages`, `web.draw_pages`, on by default). The web
  view is kept see-through, still taking the mouse and keyboard, and the
  panel draws snapshots of it taken one after another (about ten
  milliseconds each for a full-window page), written over one Bevy `Image`.
  The page then clips, rounds and fades like any panel, sits under menus and
  dragged tabs, and gets the app's screen effects. A page that stops changing
  is looked at twenty times a second until it changes again. Snapshots, the
  page's news (its title, address and loading, a press on it) and that timer
  wake the app through `UiWaker`: it may be asleep between inputs, and input
  to the page isn't the app's. While a menu,
  popup or drag is over the page the web view is hidden (see-through, it
  would take their clicks) and the last snapshot stays.
- **Overlaid pages** (the setting off). The web view shows itself: less work,
  but it covers whatever is drawn over it. A snapshot stands in for it while
  a menu or popup is open, or areas are being rearranged
  (`world.layouts().rearranging()`). `ScreenWarp`, kept current by the app,
  says how the app bends its picture (a curved-screen effect), so the web
  view can stay inside its panel as the panel appears.

Not handled: the pointer isn't corrected for a bent picture while pages are
drawn (clicks near the window's edges land slightly off under curvature),
and the app's shortcuts don't fire while the page has the keyboard.

On other platforms the panel says so, and no action or menu item is added.

```sh
cargo test -p bedtime-web
python3 tools/bed.py run tests/automation/web.py
```
