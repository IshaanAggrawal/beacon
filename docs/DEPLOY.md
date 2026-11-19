# Deploying the demo

`beacon serve` is one binary with the page embedded, so deploying it means
running one container. Nothing needs a database or a disk.

## Run the image locally

```sh
docker build -t beacon .
docker run --rm -p 8080:8080 beacon
```

Then open <http://localhost:8080>. To use LLM summaries, pass the key at run
time and never bake it into the image:

```sh
docker run --rm -p 8080:8080 --env-file .env beacon
```

Every merge to `main` also publishes the image to GitHub's container registry:

```sh
docker run --rm -p 8080:8080 ghcr.io/ishaanaggrawal/beacon:latest
```

If that pull is refused, the package is still private: open it under the
repo's **Packages** and set its visibility to public. GitHub doesn't charge for
public packages.

## Free hosting: Render

Render's free web services can run Docker images. Checked November 2026:

- 0.1 CPU, 512 MB RAM, 750 free instance hours a month.
- Spins down after 15 minutes without traffic. The next visit waits about a
  minute while it wakes up.
- Nothing on disk survives a restart. beacon doesn't need it to.

Steps:

1. Sign in at render.com with GitHub.
2. **New → Blueprint**, pick this repository. Render reads `render.yaml` and
   creates a free Docker web service called `beacon-demo`.
3. When it asks for `GROQ_API_KEY` and `GROQ_MODEL`, either paste them in or
   leave them empty to use the built-in summaries.
4. Wait for the first build. Rust takes several minutes the first time. Then
   open the `onrender.com` URL it gives you.

After that, every push to `main` redeploys automatically.

On 0.1 CPU, the first replay of each scenario takes a few seconds to simulate.
After that it's cached. Expect the first page load after a quiet spell to be
slow while the service wakes up.

## Other hosts

The image runs anywhere that runs containers. It listens on `$PORT` (default
8080) on all interfaces and has a `/healthz` endpoint.

- **Any VPS with Docker:** `docker run -d --restart unless-stopped -p 80:8080 ghcr.io/ishaanaggrawal/beacon:latest`
- **Google Cloud Run, Azure Container Apps:** both have free monthly allowances
  but need a billing account. Point them at the GHCR image.
- **Koyeb, Fly.io, Railway, Hugging Face Spaces:** free Docker offerings on these
  have been cut back or moved behind paid plans. Check their current pricing
  pages before relying on them.

## Settings

| Variable | Default | What it does |
|---|---|---|
| `PORT` | `8080` in the image | Port to listen on (all interfaces when set) |
| `GROQ_API_KEY`, `GROQ_MODEL` | unset | Turn on LLM summaries |
| `BEACON_LLM_PER_HOUR` | `120` | LLM summaries per hour across all visitors; after that, built-in summaries |
| `BEACON_MAX_SCENARIOS` | `8` | Simulated scenarios kept in memory (a few MB each) |

## Before making it public

- **Your Groq key is spent by visitors.** Every evidence card a visitor opens
  can call the LLM. `BEACON_LLM_PER_HOUR` caps that; `render.yaml` sets it to 60.
  Leave the key unset if you don't want visitors using it at all.
- **Memory is bounded.** Seeds are limited to 1–999 and only
  `BEACON_MAX_SCENARIOS` scenarios are kept, least recently used dropped first.
- **There's no login.** It's a read-only demo of simulated data; nothing
  visitors do changes anything.
