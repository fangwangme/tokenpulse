//go:build linux

package usage

import (
	"io/fs"
	"syscall"
	"time"
)

func arrivalDateFromInfo(info fs.FileInfo) *time.Time {
	modified := info.ModTime()
	if st, ok := info.Sys().(*syscall.Stat_t); ok {
		changed := time.Unix(st.Ctim.Sec, st.Ctim.Nsec)
		if changed.After(modified) {
			modified = changed
		}
	}
	t := localDate(modified)
	return &t
}

func localDate(t time.Time) time.Time {
	y, m, d := t.Local().Date()
	return time.Date(y, m, d, 0, 0, 0, 0, time.Local)
}
