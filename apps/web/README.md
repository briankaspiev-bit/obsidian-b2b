# Obsidian B2B website

Landing page for Obsidian B2B: hero booth, how remote B2B works, the three modes (Practice, Live, Open Decks) and an early-access waitlist.

Plain static files, no build step: `index.html`, `styles.css`, `main.js`. Colours and fonts match `apps/desktop/ui/src/styles/tokens.css`.

## Run locally

```
cd apps/web
python3 -m http.server 8080
# open http://localhost:8080
```

## Waitlist

The form is a placeholder. It validates the email, shows a confirmation and keeps entries in the visitor's own browser (`localStorage`), so nothing reaches us yet. Hooking it up to a real list (a form service or our own backend) is a separate decision, along with domain and hosting.
