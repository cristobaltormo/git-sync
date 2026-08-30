package forge

import (
	"testing"

	"github.com/cristobaltormo/git-sync/internal/logx"
)

func TestSigIgnoresUpdateTimeButNotFields(t *testing.T) {
	a := &Repo{ID: 1, Owner: "o", Name: "x", Updated: "2026-01-01"}
	b := &Repo{ID: 1, Owner: "o", Name: "x", Updated: "2030-01-01"}
	c := &Repo{ID: 1, Owner: "o", Name: "x", Private: true}
	if a.Sig() != b.Sig() {
		t.Error("the update time is not part of the signature")
	}
	if a.Sig() == c.Sig() {
		t.Error("visibility must change the signature")
	}
}

func TestPagesAreNotDecodedTwice(t *testing.T) {
	var c pageCache
	n := 0
	decode := func([]byte) ([]*Repo, error) { n++; return []*Repo{{ID: 1}}, nil }
	a, _ := c.decode("k", []byte(`[1]`), decode)
	b, _ := c.decode("k", []byte(`[1]`), decode)
	if &a[0] != &b[0] || n != 1 {
		t.Fatalf("identical bytes must reuse the decoded page (decoded %d times)", n)
	}
	c.decode("k", []byte(`[2]`), decode)
	if n != 2 {
		t.Fatalf("changed bytes must be decoded again (decoded %d times)", n)
	}
}

func TestSecretsAreHiddenFromLogs(t *testing.T) {
	logx.Hide("supersecretvalue")
	if got := logx.Clean("boom supersecretvalue boom"); got != "boom *** boom" {
		t.Fatalf("got %q", got)
	}
}
