package config

import (
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strings"
)

var (
	headerRe = regexp.MustCompile(`^\s*\[([A-Za-z0-9_.-]+)\]\s*(#.*)?$`)
	arrayRe  = regexp.MustCompile(`^\s*\[\[`)
)

func keyLine(key string) *regexp.Regexp {
	return regexp.MustCompile(`^\s*` + regexp.QuoteMeta(key) + `\s*=`)
}

func setLiteral(lines []string, section, key, literal string) []string {
	start, end := -1, len(lines)
	for i, l := range lines {
		if m := headerRe.FindStringSubmatch(l); m != nil && !arrayRe.MatchString(l) {
			if start >= 0 {
				end = i
				break
			}
			if m[1] == section {
				start = i
			}
		}
	}
	entry := key + " = " + literal
	if start < 0 {
		if len(lines) > 0 && strings.TrimSpace(lines[len(lines)-1]) != "" {
			lines = append(lines, "")
		}
		return append(lines, "["+section+"]", entry)
	}
	re := keyLine(key)
	for i := start + 1; i < end; i++ {
		if !re.MatchString(lines[i]) {
			continue
		}
		stop := i + 1
		if v := strings.TrimSpace(lines[i][strings.Index(lines[i], "=")+1:]); strings.HasPrefix(v, "[") && !strings.Contains(v, "]") {
			for stop < end && !strings.Contains(lines[stop], "]") {
				stop++
			}
			stop++
		}
		out := append([]string{}, lines[:i]...)
		out = append(out, entry)
		return append(out, lines[min(stop, len(lines)):]...)
	}
	at := end
	for at > start+1 && strings.TrimSpace(lines[at-1]) == "" {
		at--
	}
	out := append([]string{}, lines[:at]...)
	out = append(out, entry)
	return append(out, lines[at:]...)
}

func SetValues(path string, kv map[string]string) error {
	raw, err := os.ReadFile(path)
	if err != nil {
		return Errorf("cannot read %s: %v", path, err)
	}
	text := strings.ReplaceAll(string(raw), "\r\n", "\n")
	lines := strings.Split(strings.TrimRight(text, "\n"), "\n")
	order := make([]string, 0, len(kv))
	for k := range kv {
		order = append(order, k)
	}
	sort.Strings(order)
	for _, name := range order {
		sec, key, _ := strings.Cut(name, ".")
		lines = setLiteral(lines, sec, key, kv[name])
	}
	out := strings.Join(lines, "\n") + "\n"
	if strings.Contains(string(raw), "\r\n") {
		out = strings.ReplaceAll(out, "\n", "\r\n")
	}
	st, err := os.Stat(path)
	if err != nil {
		return err
	}
	tmp, err := os.CreateTemp(filepath.Dir(path), ".config-*.toml")
	if err != nil {
		return Errorf("cannot write in %s: %v", filepath.Dir(path), err)
	}
	defer os.Remove(tmp.Name())
	if _, err := tmp.WriteString(out); err != nil {
		tmp.Close()
		return err
	}
	tmp.Close()
	if _, err := Load(tmp.Name()); err != nil {
		return Errorf("that would make the config invalid, nothing was changed: %s", strings.TrimPrefix(err.Error(), tmp.Name()+": "))
	}
	if err := os.Chmod(tmp.Name(), st.Mode().Perm()); err != nil && !isWindows() {
		return err
	}
	keepOwner(tmp.Name(), st)
	return os.Rename(tmp.Name(), path)
}
