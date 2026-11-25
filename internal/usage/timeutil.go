package usage

import (
	"fmt"
	"io/fs"
	"time"
)

func fmtSscan(value string, out *int64) (int, error) {
	return fmt.Sscan(value, out)
}

// arrivalDateOf is the local date of the most recent moment the file could
// have gained content we have not read yet: the later of modification time
// and inode change time.
func arrivalDateOf(info fs.FileInfo) *time.Time {
	return arrivalDateFromInfo(info)
}
