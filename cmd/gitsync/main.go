package main

import (
	"os"
	"runtime/debug"

	"github.com/cristobaltormo/git-sync/internal/cli"
)

var version = "dev"

func main() {
	if version == "dev" {
		if info, ok := debug.ReadBuildInfo(); ok && info.Main.Version != "" && info.Main.Version != "(devel)" {
			version = info.Main.Version
		}
	}
	os.Exit(cli.Run(version, os.Args[1:]))
}
