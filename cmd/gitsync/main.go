package main

import (
	"os"

	"github.com/cristobaltormo/git-sync/internal/cli"
)

var version = "dev"

func main() {
	os.Exit(cli.Run(version, os.Args[1:]))
}
