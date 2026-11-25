//go:build !linux && !darwin

package usage

import (
	"io/fs"
	"time"
)

func arrivalDateFromInfo(info fs.FileInfo) *time.Time {
	t := localDate(info.ModTime())
	return &t
}

func localDate(t time.Time) time.Time {
	y, m, d := t.Local().Date()
	return time.Date(y, m, d, 0, 0, 0, 0, time.Local)
}
