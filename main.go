package main

import (
	"os"

	"github.com/fangwangme/tokenpulse/internal/cli"
)

func main() {
	os.Exit(cli.Main(os.Args[1:]))
}
