package tui

import "strings"

func (m *model) renderHeader(width int) string {
	c := newCanvas(width, 4)
	m.drawHeader(c, rect{x: 0, y: 0, w: width, h: 4})
	return strings.Join(c.lines(), "\n")
}

func (m *model) renderSettings(width int) string {
	c := newCanvas(width, 30)
	m.drawSettings(c, rect{x: 0, y: 0, w: width, h: 30})
	return strings.Join(c.lines(), "\n")
}

func (m *model) renderQuota(width int) string {
	c := newCanvas(width, 40)
	m.drawQuota(c, rect{x: 0, y: 0, w: width, h: 40})
	return strings.Join(c.lines(), "\n")
}

func (m *model) renderKeeper(width int) string {
	c := newCanvas(width, 40)
	m.drawKeeper(c, rect{x: 0, y: 0, w: width, h: 40})
	return strings.Join(c.lines(), "\n")
}
