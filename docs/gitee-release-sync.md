# Gitee Release Sync

Use `scripts/sync-gitee-release.mjs` to copy GitHub release assets to a Gitee release.

The script is dry-run by default. It downloads GitHub release packages and prints
what it would upload. It only writes to Gitee when `--execute` is present.

## Quick Run

```bash
cd /Users/weilai/Documents/log-search
node scripts/sync-gitee-release.mjs --tag v0.1.0 --download-timeout-ms 600000
node scripts/sync-gitee-release.mjs --tag v0.1.0 --download-timeout-ms 600000 --execute
```

The first command is a dry run. Check that the asset list looks right, then run
the second command to update Gitee.

## Token

The script reads `GITEE_ACCESS_TOKEN` from `.env.local`, so you do not need to
export it every time.

```bash
cd /Users/weilai/Documents/log-search
test -f .env.local && echo "token file exists"
```

If the file is missing, create it like this:

```bash
cat > .env.local
GITEE_ACCESS_TOKEN=your-token
```

Press `Ctrl-D` after the token line. `.env.local` is ignored by git.

## Step By Step

Dry run:

```bash
cd /Users/weilai/Documents/log-search
node scripts/sync-gitee-release.mjs --tag v0.1.0
```

Downloaded files are stored in:

```text
dist/gitee-release/v0.1.0/
```

If the network is slow, increase the download timeout:

```bash
cd /Users/weilai/Documents/log-search
node scripts/sync-gitee-release.mjs --tag v0.1.0 --download-timeout-ms 600000
```

Sync to Gitee:

```bash
cd /Users/weilai/Documents/log-search
node scripts/sync-gitee-release.mjs --tag v0.1.0 --download-timeout-ms 600000 --execute
```

The script will create or update the Gitee release, write a Chinese release
description, delete same-name old assets, remove any old `checksums.txt` asset,
and upload the GitHub release packages.

## Change Version

For another release, replace the tag:

```bash
cd /Users/weilai/Documents/log-search
node scripts/sync-gitee-release.mjs --tag v0.2.0 --download-timeout-ms 600000
node scripts/sync-gitee-release.mjs --tag v0.2.0 --download-timeout-ms 600000 --execute
```

Gitee requires a release target ref when creating a release. The script uses `main`
by default. Override it only when the tag should point at another branch or commit:

```bash
node scripts/sync-gitee-release.mjs --tag v0.1.0 --target-commitish main --execute
```
