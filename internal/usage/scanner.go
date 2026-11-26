package usage

import (
	"io/fs"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"time"
)

// DiscoverFiles walks root for files with one of extensions, keeping only
// files whose arrival date (the later of mtime and ctime) is on or after since
// when since is set. ctime catches a restored backup whose mtime is old.
func DiscoverFiles(root string, extensions []string, since *time.Time) []string {
	if root == "" {
		return nil
	}
	if _, err := os.Stat(root); err != nil {
		return nil
	}
	var files []string
	_ = filepath.WalkDir(root, func(path string, entry fs.DirEntry, err error) error {
		if err != nil || entry.IsDir() {
			return nil
		}
		ext := strings.TrimPrefix(filepath.Ext(path), ".")
		match := false
		for _, want := range extensions {
			if ext == want {
				match = true
				break
			}
		}
		if !match {
			return nil
		}
		if since != nil {
			if info, ierr := entry.Info(); ierr == nil {
				arrival := arrivalDateOf(info)
				if arrival == nil || arrival.Before(*since) {
					return nil
				}
			}
		}
		files = append(files, path)
		return nil
	})
	sort.Strings(files)
	return files
}

func parseRFC3339Millis(value string) (int64, bool) {
	if t, err := time.Parse(time.RFC3339Nano, value); err == nil {
		return t.UnixMilli(), true
	}
	return 0, false
}

// ParseTimestampString accepts RFC3339 or a Unix timestamp in seconds or
// milliseconds.
func ParseTimestampString(value string) (int64, bool) {
	if ms, ok := parseRFC3339Millis(value); ok {
		return ms, true
	}
	var numeric int64
	if _, err := fmtSscan(value, &numeric); err == nil {
		if numeric >= 1_000_000_000_000 {
			return numeric, true
		}
		return numeric * 1000, true
	}
	return 0, false
}

// ParseFilesParallel parses files concurrently, keeping results in file order.
func ParseFilesParallel[T any](files []string, parse func(string) []T) []T {
	if len(files) == 0 {
		return nil
	}
	type result struct {
		index int
		items []T
	}
	workers := 8
	if len(files) < workers {
		workers = len(files)
	}
	jobs := make(chan int)
	results := make(chan result, len(files))
	for w := 0; w < workers; w++ {
		go func() {
			for idx := range jobs {
				results <- result{index: idx, items: parse(files[idx])}
			}
		}()
	}
	go func() {
		for i := range files {
			jobs <- i
		}
		close(jobs)
	}()
	collected := make([][]T, len(files))
	for i := 0; i < len(files); i++ {
		r := <-results
		collected[r.index] = r.items
	}
	var out []T
	for _, items := range collected {
		out = append(out, items...)
	}
	return out
}
