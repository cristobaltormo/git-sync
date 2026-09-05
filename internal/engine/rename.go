package engine

import (
	"fmt"
	"strings"

	"github.com/cristobaltormo/git-sync/internal/logx"
)

func (s *repoSync) rename() error {
	if !s.managed || (s.ent.GHOwner == s.owner && s.ent.GHName == s.name) {
		return nil
	}
	if !strings.EqualFold(s.ent.GHOwner, s.owner) {
		logx.Warnf("%s: moved to another account; the old GitHub copy %s/%s is left as it is",
			s.repo.Full(), s.ent.GHOwner, s.ent.GHName)
		s.managed, s.has, s.ent = false, false, Entry{}
		return nil
	}
	old, err := s.e.tgt.Get(s.ent.GHOwner, s.ent.GHName)
	if err != nil {
		return err
	}
	if old != nil {
		if !strings.EqualFold(s.ent.GHName, s.name) {
			clash, err := s.e.tgt.Get(s.owner, s.name)
			if err != nil {
				return err
			}
			if clash != nil {
				return &blockedError{fmt.Sprintf("cannot rename %s/%s to %s: that name already exists on GitHub",
					s.ent.GHOwner, s.ent.GHName, s.name)}
			}
		}
		if err := s.e.tgt.Patch(s.ent.GHOwner, s.ent.GHName, map[string]any{"name": s.name}); err != nil {
			return err
		}
		s.note("renamed %s -> %s", s.ent.GHName, s.name)
	}
	gone := old == nil
	s.e.state.update(s.repo.ID, false, func(x *Entry) {
		x.GHName = s.name
		if gone {
			x.GH = nil
		}
	})
	s.ent.GHName = s.name
	if gone {
		s.ent.GH = nil
	}
	return nil
}
