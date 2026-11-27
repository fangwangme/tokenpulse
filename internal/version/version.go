// Package version holds the single version source for the Go program.
package version

import "os"

// Version is the release version. npm/scripts/build-packages.mjs reads it
// from this file, and the release tag must match it.
const Version = "0.8.3"

// String is the version as every surface shows it: the TUI header, --version,
// /api/state and the phone header. A test run is marked so a stray production
// run is impossible to confuse with one.
func String() string {
	if os.Getenv("TOKENPULSE_ENV") == "test" {
		return Version + " (test)"
	}
	return Version
}

// IsTest reports whether this process is a test run.
func IsTest() bool { return os.Getenv("TOKENPULSE_ENV") == "test" }
