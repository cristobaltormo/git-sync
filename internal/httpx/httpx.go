package httpx

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"strconv"
	"time"

	"github.com/cristobaltormo/git-sync/internal/logx"
)

type Response struct {
	Status int
	Header http.Header
	Body   []byte
}

func (r *Response) OK() bool { return r.Status >= 200 && r.Status < 300 }

func (r *Response) Text() string {
	s := string(r.Body)
	if len(s) > 300 {
		s = s[:300]
	}
	return s
}

func (r *Response) Decode(v any) error {
	if len(r.Body) == 0 {
		return fmt.Errorf("empty response")
	}
	return json.Unmarshal(r.Body, v)
}

type APIError struct {
	Status int
	Msg    string
}

func (e *APIError) Error() string { return fmt.Sprintf("HTTP %d: %s", e.Status, e.Msg) }

func Fail(r *Response, context string) error {
	msg := r.Text()
	if context != "" {
		msg = context + ": " + msg
	}
	return &APIError{Status: r.Status, Msg: msg}
}

type Client struct {
	http    *http.Client
	headers map[string]string
	retries int
	sleep   func(time.Duration)
}

func New(headers map[string]string) *Client {
	tr := &http.Transport{
		Proxy:               http.ProxyFromEnvironment,
		MaxIdleConns:        2,
		MaxIdleConnsPerHost: 1,
		IdleConnTimeout:     30 * time.Second,
		TLSHandshakeTimeout: 15 * time.Second,
	}
	return &Client{
		http:    &http.Client{Transport: tr, Timeout: 60 * time.Second},
		headers: headers, retries: 3, sleep: time.Sleep,
	}
}

func (c *Client) SetSleep(f func(time.Duration)) { c.sleep = f }

func (c *Client) Do(method, url string, body any) (*Response, error) {
	var payload []byte
	if body != nil {
		var err error
		if payload, err = json.Marshal(body); err != nil {
			return nil, err
		}
	}
	delay := time.Second
	var lastErr error
	for attempt := 0; attempt <= c.retries; attempt++ {
		req, err := http.NewRequest(method, url, bytes.NewReader(payload))
		if err != nil {
			return nil, err
		}
		for k, v := range c.headers {
			req.Header.Set(k, v)
		}
		if payload != nil {
			req.Header.Set("Content-Type", "application/json")
		}
		resp, err := c.http.Do(req)
		if err != nil {
			lastErr = err
			if attempt < c.retries {
				c.sleep(delay)
				delay *= 3
				continue
			}
			break
		}
		data, _ := io.ReadAll(io.LimitReader(resp.Body, 32<<20))
		resp.Body.Close()
		r := &Response{Status: resp.StatusCode, Header: resp.Header, Body: data}
		if r.Status >= 500 && r.Status != 501 && attempt < c.retries {
			c.sleep(delay)
			delay *= 3
			continue
		}
		if (r.Status == 403 || r.Status == 429) && attempt < c.retries && c.rateWait(r) {
			continue
		}
		return r, nil
	}
	return nil, fmt.Errorf("%s %s: %v", method, logx.Clean(url), lastErr)
}

func (c *Client) rateWait(r *Response) bool {
	wait := -1
	if ra := r.Header.Get("Retry-After"); ra != "" {
		if n, err := strconv.Atoi(ra); err == nil {
			wait = n
		}
	} else if r.Header.Get("X-RateLimit-Remaining") == "0" {
		if reset, err := strconv.ParseInt(r.Header.Get("X-RateLimit-Reset"), 10, 64); err == nil {
			wait = max(int(reset-time.Now().Unix()), 1)
		}
	}
	if wait < 0 || wait > 120 {
		return false
	}
	logx.Warnf("rate limited, waiting %ds", wait)
	c.sleep(time.Duration(wait+1) * time.Second)
	return true
}
