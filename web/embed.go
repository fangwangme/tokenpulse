// Package web embeds the built phone app.
package web

import "embed"

//go:embed dist
var FS embed.FS
