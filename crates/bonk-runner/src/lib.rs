// This empty lib target exists so bonk-cli can declare a Cargo dependency on
// bonk-runner. release-plz follows the dependency graph: when bonk-runner
// changes and its version is bumped, bonk-cli is bumped too — keeping the
// single workspace version in sync and updating the CHANGELOG.
