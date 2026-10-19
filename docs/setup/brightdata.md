# Bright Data setup

How to get the two Bright Data values mex-scrape needs, put them in place, and check they work. About 10 minutes. Bright Data's control panel is confusing, so this guide names every screen and warns where it misleads.

Last checked against the Bright Data control panel: 2026-10-19.

## What you need at the end

Two values, and nothing else:

| Variable | What it is | Example shape |
| --- | --- | --- |
| `BRIGHTDATA_API_KEY` | Your **account API key**. Lets the app call Bright Data's API. | a long random string (about 36 characters) |
| `BRIGHTDATA_UNLOCKER_ZONE` | The **name you gave your Web Unlocker zone**. Tells Bright Data which product and settings to use. | `mex_unlocker` |

> **Trap:** after you create the zone, Bright Data shows a "credentials" box with a **username, password, host and port**. Those are *proxy* credentials. **They are not the API key** and mex-scrape does not use them. Ignore that box.

## Step 1: Create an account

1. Sign up at <https://brightdata.com>.
2. If you sign up with Gmail or GitHub, Bright Data asks for a payment method before it lets you create anything. That is identity verification; the first 5,000 Web Unlocker requests each month are free.

You do **not** need KYC (company verification) to start. Without it, Bright Data refuses pages that the store's robots.txt forbids; see [brightdata-access.md](../brightdata-access.md) for what that means per retailer.

## Step 2: Create the Web Unlocker zone

1. Open the control panel: <https://brightdata.com/cp>.
2. In the left menu choose **Web Access**, then **Create API**.
   - **Not** "Proxies" / "Residential" / "Browser API" for this step. Those are billed per GB (about $8/GB). The app uses the Browser API only for a few JavaScript-only pages; that zone is optional and has its own section below ([Optional: Browser API zone](#optional-browser-api-zone)).
3. Pick **Web Unlocker API** → **Continue**.
4. Name it, for example `mex_unlocker`. **The name cannot be changed later.** Write it down: this is `BRIGHTDATA_UNLOCKER_ZONE`.
5. Leave **Premium domains** unticked unless a retailer needs it (it adds about $1 per 1,000 requests).
6. Click **Add API** (or **Add payment method** if you have not added one).

### Zone settings: leave these off

In the zone's **Configuration → Advanced settings**:

| Setting | Set to | Why |
| --- | --- | --- |
| Custom headers & cookies | **off** | When on, Bright Data bills failed requests too. The app does not send custom headers. |
| Manual 'expect' elements | **off** | The app does not use it. |
| Asynchronous requests | **off** | The app waits for each response directly. |
| Usage limit | **on**, small cap (e.g. $2/month to start) | Protects you from a surprise bill. Raise it when real use starts. |

## Step 3: Create the API key

The key is **not** on the zone page. It lives in account settings:

1. Open <https://brightdata.com/cp/setting/users>.
2. Create an API key (if asked, give it permission to use your zones; no admin rights are needed).
3. Copy it right away: Bright Data may not show it again. This is `BRIGHTDATA_API_KEY`.

Each person (you, a teammate) should create **their own key**. Never paste a key into chat, email, an issue or a commit. If a key leaks, delete it on the same page and make a new one.

## Step 4: Put the values in `.env`

`.env` is git-ignored, so it never gets committed. Nothing loads it automatically; you load it in the shell before running a command.

1. If you do not have one yet, copy the template: `cp .env.example .env`.
2. Open `.env` and find these two lines:
   ```
   # BRIGHTDATA_API_KEY=
   # BRIGHTDATA_UNLOCKER_ZONE=
   ```
3. Remove the `# ` at the start of **both** lines and paste each value after `=`. No quotes, no spaces:
   ```
   BRIGHTDATA_API_KEY=<your key>
   BRIGHTDATA_UNLOCKER_ZONE=mex_unlocker
   ```
   A line that is uncommented but left empty stops the worker on purpose.

## Step 5: Check it works

In Git Bash (Windows) or any bash shell, from the project folder:

```bash
set -a && . ./.env && set +a
cargo run -p worker --bin access_check            # free: robots.txt only, no Bright Data request
cargo run -p worker --bin access_check -- --live  # paid: at most 12 requests (inside the free 5,000)
```

The live run rewrites [docs/brightdata-access.md](../brightdata-access.md) with a dated table. Every cell showing `allowed`, or `blocked by robots.txt`, means the setup works. Each `--live` run uses one request per allowed page (8 as of 2026-10-19).

## Optional: Browser API zone

Only needed for pages that build their products with JavaScript. Today that is Sears MX and Sanborns category listings. Everything else uses the Web Unlocker zone above. Last checked: 2026-10-19.

1. In the control panel choose **Web Access APIs** → **Create API** → **Browser API** → **Continue**.
2. Name it, for example `mex_browser`. **The name cannot be changed later.** This is `BRIGHTDATA_BROWSER_ZONE`.
3. Leave **CAPTCHA Solver** on (no extra cost). Leave **Custom headers & cookies** off: the app does not send any.
4. Set a **usage limit** (e.g. $2/month) in Advanced settings, or on the zone page after creating it.
5. Click **Add API**.
6. In `.env`, add `BRIGHTDATA_BROWSER_ZONE=mex_browser` (no `#`, no quotes). The API key from Step 3 is reused.

> **Trap:** the "Test your API" screen shows a `wss://brd-customer-…:********@brd.superproxy.io:9222` address with an embedded password. **Ignore it and never paste it anywhere.** The app looks up the customer id and zone password itself through the API key. If your API key is limited to certain zones, give it access to the new zone at <https://brightdata.com/cp/setting/users>.

**Cost:** about $8/GB of data the browser loads. The app blocks images, fonts, video, stylesheets and ad trackers, so a page costs mostly its HTML and scripts. A rendered Sears listing is about 1 MB of HTML. The billed bytes per page have not been measured yet; check the zone's statistics after a few captures.

**Check it:** capture one Sears listing into a scratch folder. The exact command is in [sears_mx.md](../modules/retailers/sears_mx.md#why-the-browser-api). The output should end with `status 200, … bytes`. `--browser needs BRIGHTDATA_BROWSER_ZONE` means `.env` was not loaded or the line still starts with `#`.

## Troubleshooting

| You see | Meaning | Fix |
| --- | --- | --- |
| `No Bright Data API key: set BRIGHTDATA_API_KEY` or `No Web Unlocker zone specified: set BRIGHTDATA_UNLOCKER_ZONE` | `.env` was not loaded, or the line still starts with `#` | Run the `set -a && . ./.env && set +a` line again in the same terminal; check the `#` is gone. |
| `BRIGHTDATA_API_KEY is empty or only whitespace` (or `BRIGHTDATA_UNLOCKER_ZONE ...`) | The variable is set but has no value, e.g. `BRIGHTDATA_API_KEY=` with nothing after `=` | Paste the value after `=` in `.env`, then run the `set -a && . ./.env && set +a` line again. |
| Every cell `error`, message mentions 401 / 403 / auth | Wrong key, or you pasted the proxy password | Create a key at <https://brightdata.com/cp/setting/users> and paste that. |
| Every cell `error`, message mentions the zone | Zone name typo, or the zone is not Web Unlocker | Copy the exact name from **Web Access**. |
| One cell `error` with "http2" / "stream error" | Network hiccup | Run the live check again. |
| `blocked by robots.txt` | Normal without KYC | Nothing to fix; KYC lifts it. See [compliance/brightdata-kyc.md](../compliance/brightdata-kyc.md). |
| `blocked by policy` | Bright Data needs compliance approval for that site | Same as above: part of KYC. |

## Cost at a glance

- Web Unlocker: 5,000 requests/month free, then about $1.50 per 1,000 successful requests (retail sites may add about $1 per 1,000 as premium domains).
- The app's own caps: 20 runs and 500 pages per tenant per day, 50 per run. Worst case is roughly $25/month per tenant after the free tier.
- Prices change; check <https://brightdata.com/pricing> before quoting a client.

## Production

On a server, set the same two variables in the host's secret store (Docker/compose secrets, Fly/Railway secrets, AWS/GCP Secret Manager) for the **worker only**. The API and web app never need them, and they must never be in a `NEXT_PUBLIC_*` variable.

## Notes for in-app onboarding (later)

Friction found while setting this up; each is a candidate for the app to hide or explain:

1. Two values live in **two different places** (zone name under Web Access, API key under account settings). The zone page never mentions the API key.
2. The zone shows **proxy credentials** that look like the thing you need but are not. Easy to paste the wrong one.
3. The left menu mixes per-GB proxy products with per-request APIs; the first page many people land on is the proxy page.
4. Gmail/GitHub sign-ups hit a payment step before anything works, with no explanation that the free tier still applies.
5. A zone name is permanent, so typos are forever.
6. Nothing confirms success until you send a request.

Ideas: a settings screen with one field per value and a link to the exact page for each; a "Test connection" button that runs the free robots check and one live request and shows the matrix; validate the key shape and reject values that look like a proxy password (`brd-customer-...`).

Clients bringing their own Bright Data account would need encrypted per-tenant key storage; that is not built yet.
