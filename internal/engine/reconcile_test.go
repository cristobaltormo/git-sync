package engine

import (
	"testing"

	"github.com/cristobaltormo/git-sync/internal/forge"
)

func TestFingerprintSeesEveryMirroredField(t *testing.T) {
	base := func() []*forge.Repo { return []*forge.Repo{mk(1, "a"), mk(2, "b")} }
	f0 := fingerprint(base())
	eq(t, f0, fingerprint([]*forge.Repo{mk(2, "b"), mk(1, "a")}))
	changes := map[string]func(*forge.Repo){
		"visibility":  func(r *forge.Repo) { r.Private = false },
		"description": func(r *forge.Repo) { r.Description = "x" },
		"website":     func(r *forge.Repo) { r.Website = "https://x.example" },
		"archived":    func(r *forge.Repo) { r.Archived = true },
		"branch":      func(r *forge.Repo) { r.DefaultBranch = "trunk" },
		"topics":      func(r *forge.Repo) { r.Topics = []string{"go"} },
		"rename":      func(r *forge.Repo) { r.Name = "renamed" },
		"push":        func(r *forge.Repo) { r.Updated = "2031-01-01T00:00:00Z" },
	}
	for name, mutate := range changes {
		repos := base()
		mutate(repos[0])
		isTrue(t, fingerprint(repos) != f0, name+" must change the fingerprint")
	}
	isTrue(t, fingerprint(base()[:1]) != f0, "a deleted repo must change the fingerprint")
}
