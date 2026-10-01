package cli

import (
	"context"
	"flag"
	"os"
	"os/signal"
	"sync"
	"syscall"
	"time"

	"github.com/cristobaltormo/git-sync/internal/config"
	"github.com/cristobaltormo/git-sync/internal/logx"
	"github.com/cristobaltormo/git-sync/internal/webhook"
)

func cmdRun(cfgPath string, args []string) (int, error) {
	var dry *bool
	path, _, err := flags("run", cfgPath, args, func(fs *flag.FlagSet) {
		dry = fs.Bool("dry-run", false, "log what would happen, change nothing")
	})
	if err != nil {
		return 2, err
	}
	file := config.Find(path)
	cfg, err := config.Load(file)
	if err != nil {
		return 2, err
	}
	if *dry {
		cfg.Sync.DryRun = true
	}
	a, err := build(cfg)
	if err != nil {
		return 2, err
	}
	if err := os.MkdirAll(cfg.Paths.StateDir, 0o755); err != nil {
		return 2, config.Errorf("cannot create %s: %v", cfg.Paths.StateDir, err)
	}
	a.eng.Start()
	handler := webhook.Handler(a.src, func() string { return a.eng.Config().Listen.Secret }, a.eng.HandleEvent)
	srv, err := webhook.Listen(cfg.Listen.Host, cfg.Listen.Port, handler)
	if err != nil {
		return 2, err
	}
	suffix := ""
	if a.eng.Dry() {
		suffix = " [dry run]"
	}
	logx.Infof("gitsync %s up: %s %s -> GitHub, %d account(s), webhooks on %s%s", Version,
		cfg.Source.Type, cfg.SourceURL(), len(cfg.Accounts), config.HostPort(cfg.Listen.Host, cfg.Listen.Port), suffix)
	a.eng.SetupHooks(false)

	ctx, stop := signal.NotifyContext(context.Background(), syscall.SIGINT, syscall.SIGTERM)
	defer stop()
	r := &reloader{file: file, cur: cfg, a: a, stamp: cfg.Stamp()}
	a.eng.OnTick = r.checkFiles
	go r.onHUP()
	go a.eng.PollLoop(ctx)

	<-ctx.Done()
	logx.Infof("stopping")
	sc, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	srv.Shutdown(sc)
	a.eng.Shutdown()
	return 0, nil
}

type reloader struct {
	file  string
	cur   *config.Config
	a     *app
	mu    sync.Mutex
	stamp string
}

func (r *reloader) reload() {
	r.mu.Lock()
	defer r.mu.Unlock()
	st := r.cur.Stamp()
	next, err := config.Load(r.file)
	r.stamp = st
	if err != nil {
		logx.Errorf("reload failed, keeping the old config: %v", err)
		return
	}
	if next.Listen.Host != r.cur.Listen.Host || next.Listen.Port != r.cur.Listen.Port {
		logx.Warnf("listen address changed: restart needed for that part")
	}
	r.a.eng.Reload(next)
	logx.Infof("config reloaded")
}

func (r *reloader) checkFiles() {
	r.mu.Lock()
	changed := r.stamp != r.cur.Stamp()
	r.mu.Unlock()
	if changed {
		r.reload()
	}
}

func (r *reloader) onHUP() {
	hup := make(chan os.Signal, 1)
	signal.Notify(hup, syscall.SIGHUP)
	for range hup {
		r.reload()
	}
}
