package config

import (
	"encoding/json"
	"fmt"
	"reflect"
	"strconv"
	"strings"
)

type Key struct {
	Name   string
	Kind   reflect.Kind
	Secret bool
	field  []int
}

var sections = []string{"source", "github", "listen", "sync", "filter", "hooks", "paths", "log", "notify", "pull_requests"}

var sectionField = map[string]string{
	"source": "Source", "github": "GitHub", "listen": "Listen", "sync": "Sync", "filter": "Filter",
	"hooks": "Hooks", "paths": "Paths", "log": "Log", "notify": "Notify", "pull_requests": "Pulls",
}

func Keys() []Key {
	var out []Key
	t := reflect.TypeOf(Config{})
	for _, sec := range sections {
		sf, _ := t.FieldByName(sectionField[sec])
		for i := 0; i < sf.Type.NumField(); i++ {
			f := sf.Type.Field(i)
			name := f.Tag.Get("toml")
			out = append(out, Key{
				Name: sec + "." + name, Kind: f.Type.Kind(), field: []int{sf.Index[0], i},
				Secret: name == "token" || name == "secret",
			})
		}
	}
	return out
}

func FindKey(name string) (Key, bool) {
	name = strings.ToLower(strings.TrimSpace(name))
	for _, k := range Keys() {
		if k.Name == name {
			return k, true
		}
	}
	return Key{}, false
}

func (k Key) value(c *Config) reflect.Value { return reflect.ValueOf(c).Elem().FieldByIndex(k.field) }

func (k Key) Get(c *Config) string {
	v := k.value(c)
	switch k.Kind {
	case reflect.Slice:
		parts := make([]string, v.Len())
		for i := range parts {
			parts[i] = v.Index(i).String()
		}
		return strings.Join(parts, ", ")
	case reflect.Bool:
		return strconv.FormatBool(v.Bool())
	case reflect.Int:
		return strconv.FormatInt(v.Int(), 10)
	}
	return v.String()
}

func (k Key) Hint() string {
	switch k.Kind {
	case reflect.Bool:
		return "true or false"
	case reflect.Int:
		return "a number"
	case reflect.Slice:
		return "a comma separated list"
	}
	return "text"
}

func (k Key) Literal(vals []string) (string, error) {
	switch k.Kind {
	case reflect.Bool:
		if len(vals) != 1 {
			return "", Errorf("%s takes one value (true or false)", k.Name)
		}
		switch strings.ToLower(vals[0]) {
		case "true", "yes", "on", "1":
			return "true", nil
		case "false", "no", "off", "0":
			return "false", nil
		}
		return "", Errorf("%s must be true or false", k.Name)
	case reflect.Int:
		if len(vals) != 1 {
			return "", Errorf("%s takes one number", k.Name)
		}
		n, err := strconv.Atoi(vals[0])
		if err != nil {
			return "", Errorf("%s must be a number", k.Name)
		}
		return strconv.Itoa(n), nil
	case reflect.Slice:
		var items []string
		for _, v := range vals {
			for _, p := range strings.Split(v, ",") {
				if p = strings.TrimSpace(p); p != "" {
					items = append(items, p)
				}
			}
		}
		q := make([]string, len(items))
		for i, s := range items {
			b, _ := json.Marshal(s)
			q[i] = string(b)
		}
		return "[" + strings.Join(q, ", ") + "]", nil
	}
	if len(vals) != 1 {
		return "", Errorf("%s takes one value (quote it if it has spaces)", k.Name)
	}
	b, _ := json.Marshal(vals[0])
	return string(b), nil
}

type Profile struct {
	Name, Summary string
	Set           map[string]string
}

var Profiles = []Profile{
	{
		Name:    "personal",
		Summary: "Mirror everything you own. New repositories start syncing on their own.",
		Set: map[string]string{
			"filter.new_repos": `"sync"`, "filter.scope": `"all"`, "filter.skip_forks": "false",
			"sync.visibility": "true", "sync.on_delete": `"delete"`,
		},
	},
	{
		Name:    "team",
		Summary: "For when others can add you to projects: new repositories wait for your OK, repos where you are only a collaborator are left alone.",
		Set: map[string]string{
			"filter.new_repos": `"review"`, "filter.scope": `"admin"`, "filter.skip_forks": "true",
			"sync.visibility": "true", "sync.on_delete": `"archive"`,
		},
	},
	{
		Name:    "careful",
		Summary: "Only what you pick. Nothing new is synced, the GitHub copy is never made public by itself and removals are archived.",
		Set: map[string]string{
			"filter.new_repos": `"ignore"`, "filter.scope": `"all"`, "filter.skip_forks": "true",
			"sync.visibility": "false", "sync.on_delete": `"archive"`,
		},
	},
}

func FindProfile(name string) (Profile, bool) {
	for _, p := range Profiles {
		if p.Name == strings.ToLower(name) {
			return p, true
		}
	}
	return Profile{}, false
}

func (p Profile) Describe() string { return fmt.Sprintf("%-9s %s", p.Name, p.Summary) }
