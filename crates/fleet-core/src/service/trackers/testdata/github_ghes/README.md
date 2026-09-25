# GitHub Enterprise Server golden (work graph M11.4)

The enterprise harness in `tests_github.rs` (`mod ghes`) replays the
`testdata/github/` fixtures — an enterprise instance answers the same
GraphQL shapes — through the real `gh` transport pointed at
`ghe.corp.example:8443`. Only the normalisation differs: keys, parents and
containers carry the instance's host (`ghe.corp.example/acme/api#42`). The
`url` fields are the fixtures' own and are not rewritten.

| File | What |
|---|---|
| `golden_list.json` | the normalised listing (`REGEN_TRACKER_GOLDENS=1`) |
