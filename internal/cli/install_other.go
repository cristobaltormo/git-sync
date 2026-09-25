//go:build !linux && !windows

package cli

import "github.com/cristobaltormo/git-sync/internal/config"

const noService = "install is available on Linux (systemd) and Windows; here run gitsync in the background yourself, or use Docker"

func cmdInstall(cfgPath string, args []string) (int, error) { return 2, config.Errorf(noService) }

func cmdUninstall(cfgPath string, args []string) (int, error) { return 2, config.Errorf(noService) }
