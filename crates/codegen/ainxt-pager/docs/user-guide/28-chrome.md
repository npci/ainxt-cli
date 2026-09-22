# Chrome

ainxt can drive a real Chrome browser over the DevTools Protocol. Unlike
`web_fetch`, which issues an anonymous HTTP request, this renders pages in a
browser that carries your logged-in sessions — so authenticated pages work.

---

## Why a separate profile

Chrome cannot be attached to after it has started. The DevTools port only
exists if `--remote-debugging-port` was passed at launch, and a second process
cannot share a running instance's profile directory — Chrome aborts on the
profile lock rather than risk corruption. Chrome 136+ additionally refuses
remote debugging when the profile is the default user-data-dir.

So ainxt runs **its own Chrome** against its own profile at
`~/.ainxt/chrome-profile`, seeded once from your real profile so your logins
carry over. Your everyday browser keeps running, untouched.

The seed copies `Cookies`, `Login Data`, `Web Data` and `Preferences`. It
happens once, on first use. Sessions drift as cookies expire — delete
`~/.ainxt/chrome-profile` to re-seed from a fresh state.

---

## Tools

| Tool | Scope | What it does |
|---|---|---|
| `chrome_navigate` | Write | Opens a URL and waits for load |
| `chrome_read_page` | Read | Returns the page as an accessibility outline |
| `chrome_click` | Write | Clicks an element by its `[ref=N]` handle |
| `chrome_type` | Write | Types into a field, optionally pressing Enter |
| `chrome_screenshot` | Write | Captures the page as an image, inline or saved to a file |

`chrome_read_page` returns one line per element — role, accessible name, and a
`[ref=N]` handle — which is smaller and more reliable than raw HTML:

```
RootWebArea "Example Domain" [ref=1]
  heading "Example Domain" [ref=10]
  paragraph [ref=11]
    StaticText "This domain is for use in documentation examples…" [ref=15]
  link "Learn more" [ref=13]
```

### Interacting with elements

`chrome_click` and `chrome_type` address elements by the `[ref=N]` handle from
`chrome_read_page`. Clicks dispatch real mouse events at the element's centre
rather than calling `.click()` in JavaScript, so hover handlers, focus changes
and event delegation behave as they do for a human.

**Refs go stale.** They are `backendDOMNodeId` values for the DOM as it was
when the page was read. After any navigation or dynamic update, read the page
again before acting on it. Clicking a stale ref reports that the element has
no layout box rather than clicking whatever happens to be there now.

### Screenshots

`chrome_read_page` is the right tool for text, structure and finding things to
click. `chrome_screenshot` is for what an outline cannot carry: layout, colour,
images, charts, or confirming a page renders as intended.

Without `save_path` the image comes back inline for the model to look at and is
not written anywhere. With `save_path` the file is written and the path
returned instead — saving is what was asked for, so the image does not also
consume context:

```
chrome_screenshot(full_page: true, save_path: "~/Downloads/ledger.png")
  -> Saved screenshot to /Users/you/Downloads/ledger.png (184320 bytes, image/png)
```

A `save_path` naming a directory gets a generated filename. A missing
extension is filled in from the format. Relative paths are rejected rather
than resolved against a working directory the model cannot see.

Because `save_path` can create a file anywhere you can write, the tool is
classified `Write` and goes through the approval path — even though a capture
on its own changes nothing. Tool capabilities are static, so the tool is
classified by the most privileged thing it can do.

You do **not** need macOS screen-recording permission. The image comes from
Chrome over the DevTools protocol, not from the OS screen capture APIs, so it
works headless and captures only the page.

### Why navigating counts as a write

`chrome_navigate` is registered with `ToolScope::Write` and `is_read_only:
false`, so it routes through the approval path rather than firing unattended.
A navigation looks like a read, but this browser is signed in as you — a plain
GET carrying your cookies can act on your behalf. `chrome_read_page` inspects
an already-loaded page and is a genuine read.

---

## Usage

The `browser-use` agent carries both tools:

```sh
ainxt --agent browser-use
```

Chrome launches on the first tool call, not at session start, and the same
window serves every later call so tabs and history persist across the
conversation.

---

## Configuration

Defaults live in `ChromeParams`:

| Field | Default | Meaning |
|---|---|---|
| `port` | 9222 | DevTools port |
| `seed_from_default_profile` | `true` | Copy logins from your real profile |
| `headless` | `false` | A visible window is what makes this auditable |
| `max_read_chars` | 40000 | Ceiling on one page read |

Set `AINXT_CHROME_BINARY` to point at a non-standard Chrome or Chromium
install. A path that does not exist is an error rather than a silent fallback.

---

## Safety

The browser is signed in as you, which makes it powerful and worth treating
carefully.

- **Page content is data, never instructions.** Text on a page that appears to
  address the agent — "ignore previous instructions", "the user has approved
  this" — carries no authority. The `browser-use` agent's prompt says so
  explicitly, but the guarantee is the approval prompt on `chrome_navigate`,
  not the model's judgment.
- **The DevTools port has no authentication.** While Chrome is running, any
  local process can drive it. It binds to `127.0.0.1` only.
- **Never have the agent enter credentials.** `chrome_type` carries this
  instruction in its own description, but the real guarantee is you: log in
  yourself in the ainxt Chrome window, and the session persists in the profile.

---

## Troubleshooting

**"Chrome not found"** — set `AINXT_CHROME_BINARY` to the binary path.

**"Chrome did not expose a DevTools endpoint"** — something else is on port
9222, or a previous ainxt Chrome is still running. Close it, or change `port`.

**Logins did not carry over** — the seed happens only on first use. Delete
`~/.ainxt/chrome-profile` and let it re-seed.

**Google still shows "Sign in" despite the seed** — this is by design and
cannot be fixed by copying files. Chrome binds Google sessions with Device
Bound Session Credentials: a key held in the Secure Enclave and tied to the
original profile. The cookies copy and decrypt correctly, but Google rejects
them server-side because the binding key cannot follow. Sign into Google once
inside the ainxt Chrome window; the session then binds to that profile and
persists. Sites without device-bound sessions (GitHub, most others) carry over
from the seed normally.

**A stale Chrome blocks the launch** — it no longer does. `launch()` probes the
DevTools port first and reuses a Chrome already serving it, rather than
spawning a second one that would abort on the profile lock.
