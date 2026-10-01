package forge

import (
	"encoding/json"
	"testing"
)

func TestGiteaRole(t *testing.T) {
	for body, want := range map[string]string{
		`{"id":1,"name":"a","full_name":"o/a","permissions":{"admin":true,"push":true,"pull":true}}`:   RoleAdmin,
		`{"id":1,"name":"a","full_name":"o/a","permissions":{"admin":false,"push":true,"pull":true}}`:  RoleWrite,
		`{"id":1,"name":"a","full_name":"o/a","permissions":{"admin":false,"push":false,"pull":true}}`: RoleRead,
		`{"id":1,"name":"a","full_name":"o/a"}`:                                                        "",
	} {
		var g giteaRepo
		if err := json.Unmarshal([]byte(body), &g); err != nil {
			t.Fatal(err)
		}
		if got := g.repo().Role; got != want {
			t.Errorf("%s: role %q, want %q", body, got, want)
		}
	}
}
