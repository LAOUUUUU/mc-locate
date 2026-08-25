# Publishing the mod to Modrinth & CurseForge

Publishing is automated by [`.github/workflows/publish-mods.yml`](../../.github/workflows/publish-mods.yml),
which uploads every version's jar to both sites after each GitHub release. It is
**off until you arm it**, because the accounts, the projects, and the tokens are
yours — Claude cannot create accounts, create projects, or upload on your behalf.

## One-time setup (only you can do these)

### 1. Create the projects

- **Modrinth** — <https://modrinth.com/> → *Create a project*. Set it to a mod,
  environment *client*, license MIT. Note the **project ID** (or slug) from the
  URL, e.g. `mc-locate-exporter`.
- **CurseForge** — <https://authors.curseforge.com/> → *Create Project* under
  Minecraft → Mods. CurseForge **manually reviews** new projects, so approval
  takes a little while. Once approved, note the **numeric Project ID** shown on
  the project page.

Use the description and the "which jar for which version" table from
[README.md](README.md); the icon can be any 512×512 PNG.

### 2. Get API tokens

- **Modrinth** — <https://modrinth.com/settings/pats>, scope *Create versions*
  (and *Create projects* if you want the action to create it for you).
- **CurseForge** — <https://legacy.curseforge.com/account/api-tokens>.

### 3. Add them to the repo

In **Settings → Secrets and variables → Actions**:

Secrets:
- `MODRINTH_TOKEN`
- `CURSEFORGE_TOKEN`

Variables:
- `MODRINTH_ID` — the Modrinth project id/slug
- `CURSEFORGE_ID` — the CurseForge numeric project id
- `PUBLISH_MODS` — set to `true` to arm the workflow

## After that

It runs by itself: each `git tag vX.Y.Z && git push --tags` builds the jars
(release.yml), publishes the GitHub release, and then publish-mods.yml uploads
all 14 jars — one version per supported Minecraft release — to Modrinth and
CurseForge. You can also run it by hand from the Actions tab (*Publish mod to
Modrinth & CurseForge* → *Run workflow*, giving a release tag).

To publish the current release (v0.6.0) once configured, use that manual run
with tag `v0.6.0`.

> Not yet test-run: the workflow is written against mc-publish v3's documented
> inputs, but it has never executed here because it needs your tokens. Watch the
> first run in the Actions tab and check both project pages.
