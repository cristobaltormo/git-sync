package engine

import (
	"fmt"
	"testing"
	"time"

	"github.com/cristobaltormo/git-sync/internal/config"
	"github.com/cristobaltormo/git-sync/internal/forge"
)

func TestDeleteAfterGrace(t *testing.T) {
	h := synced(t, func(c *config.Config) { c.Sync.DeleteGrace = 30 })
	delete(h.src.repos, 1)
	h.e.Reconcile(false, "scan")
	isTrue(t, h.tgt.has("alice-gh/site"), "grace not over yet")
	h.e.state.update(1, false, func(x *Entry) { x.MissingSince = time.Now().Unix() - 60 })
	h.e.Reconcile(false, "scan")
	isTrue(t, !h.tgt.has("alice-gh/site"), "should be deleted")
	_, ok := h.e.state.get(1)
	isTrue(t, !ok, "entry should be gone")
}

type hidingSource struct {
	*fakeSource
	hide int64
}

func (l *hidingSource) List(owner string) ([]*forge.Repo, error) {
	all, _ := l.fakeSource.List(owner)
	var out []*forge.Repo
	for _, r := range all {
		if r.ID != l.hide {
			out = append(out, r)
		}
	}
	return out, nil
}

func TestConfirmBeforeDelete(t *testing.T) {
	h := synced(t, nil)
	h.e.src = &hidingSource{fakeSource: h.src, hide: 1}
	h.e.Reconcile(false, "scan")
	isTrue(t, h.tgt.has("alice-gh/site"), "must not delete when the repo still exists")
}

func TestEmptyListingIsNotAMassDelete(t *testing.T) {
	h := synced(t, nil)
	h.src.set(mk(2, "other"))
	h.run(false)
	h.src.repos = map[int64]*forge.Repo{}
	h.e.Reconcile(false, "scan")
	isTrue(t, h.tgt.has("alice-gh/site") && h.tgt.has("alice-gh/other"), "nothing may be deleted")
}

func TestSingleRepoCanBeDeleted(t *testing.T) {
	h := synced(t, nil)
	h.src.repos = map[int64]*forge.Repo{}
	h.e.Reconcile(false, "scan")
	isTrue(t, !h.tgt.has("alice-gh/site"), "the only repo should be deletable")
}

func TestOwnerLookupFailureBlocksDelete(t *testing.T) {
	h := synced(t, nil)
	h.src.set(mk(2, "other"))
	h.run(false)
	delete(h.src.repos, 1)
	h.src.ownerOK = false
	h.e.Reconcile(false, "scan")
	isTrue(t, h.tgt.has("alice-gh/site"), "owner lookup failed: keep it")
}

func TestArchiveMode(t *testing.T) {
	h := synced(t, func(c *config.Config) { c.Sync.OnDelete = "archive" })
	delete(h.src.repos, 1)
	h.src.set(mk(2, "keep"))
	h.e.Reconcile(false, "scan")
	isTrue(t, h.tgt.repos["alice-gh/site"].Archived, "should be archived, not deleted")
}

func TestIgnoreMode(t *testing.T) {
	h := synced(t, func(c *config.Config) { c.Sync.OnDelete = "ignore" })
	delete(h.src.repos, 1)
	h.src.set(mk(2, "keep"))
	h.e.Reconcile(false, "scan")
	isTrue(t, !h.tgt.repos["alice-gh/site"].Archived, "must be left alone")
	_, ok := h.e.state.get(1)
	isTrue(t, !ok, "entry should be forgotten")
}

func TestDeleteLimit(t *testing.T) {
	h := newHarness(t, func(c *config.Config) { c.Sync.DeleteLimit = 2 })
	for i := int64(1); i <= 4; i++ {
		h.src.set(mk(i, fmt.Sprintf("r%d", i)))
	}
	h.run(false)
	h.src.repos = map[int64]*forge.Repo{}
	h.src.set(mk(9, "survivor"))
	h.e.Reconcile(false, "scan")
	gone := 0
	for _, k := range []string{"r1", "r2", "r3", "r4"} {
		if !h.tgt.has("alice-gh/" + k) {
			gone++
		}
	}
	eq(t, gone, 2)
}

func TestNeverDeletesForeignRepo(t *testing.T) {
	h := synced(t, nil)
	h.tgt.repos["alice-gh/site"].Owner.Login = "someone-else"
	delete(h.src.repos, 1)
	h.src.set(mk(2, "keep"))
	h.e.Reconcile(false, "scan")
	isTrue(t, h.tgt.has("alice-gh/site"), "must not delete what we do not own")
}

func TestListingFailureSkipsScan(t *testing.T) {
	h := synced(t, nil)
	h.src.listErr = fmt.Errorf("boom")
	isTrue(t, !h.e.Reconcile(false, "scan"), "reconcile should report failure")
	isTrue(t, h.tgt.has("alice-gh/site"), "nothing may be deleted on a failed listing")
}
