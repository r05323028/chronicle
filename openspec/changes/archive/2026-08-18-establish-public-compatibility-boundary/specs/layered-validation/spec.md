## ADDED Requirements

### Requirement: Release snapshots prove released state

Release validation SHALL verify more than the existence of a versioned documentation directory. For the requested release snapshot, it SHALL inspect the committed English, Traditional Chinese, and Japanese Markdown content and fail when semantic release-state contradictions claim that no public release exists or that the GitHub Release installer is only future/conditional. The guard SHALL remain a small list of release-state predicates, not an exact prose snapshot or localization equality test, and SHALL run from the release preparation path.

#### Scenario: Stale English snapshot fails

- **WHEN** a committed 0.1 snapshot says that a public release does not exist or that the installer starts with a future public release
- **THEN** release documentation validation fails even though all required snapshot directories and sidebars exist

#### Scenario: Localized stale snapshot fails

- **WHEN** a committed localized 0.1 snapshot contains the equivalent unreleased-state claim
- **THEN** release documentation validation fails and names the affected locale/file

#### Scenario: Released snapshot passes

- **WHEN** all required 0.1 snapshots present the installer as supported and retain only accurate capability limitations
- **THEN** release documentation validation passes without requiring exact prose identity across locales
