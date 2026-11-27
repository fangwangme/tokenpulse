package tui

import (
	"strings"

	"github.com/charmbracelet/lipgloss"
)

// textStyle is a comparable cell style. A canvas stores these instead of
// lipgloss.Style values so adjacent cells can share one ANSI sequence.
type textStyle struct {
	fg   lipgloss.Color
	bg   lipgloss.Color
	bold bool
}

func fgStyle(color lipgloss.Color) textStyle { return textStyle{fg: color} }
func boldStyle(color lipgloss.Color) textStyle {
	return textStyle{fg: color, bold: true}
}

type cell struct {
	text  string
	style textStyle
}

// canvas is a small cell buffer: pages place text at exact coordinates instead
// of composing nested lipgloss boxes, which keeps a full-width frame aligned.
type canvas struct {
	w, h   int
	cells  []cell
	styles map[textStyle]lipgloss.Style
}

func newCanvas(width, height int) *canvas {
	if width < 0 {
		width = 0
	}
	if height < 0 {
		height = 0
	}
	c := &canvas{w: width, h: height, styles: map[textStyle]lipgloss.Style{}}
	c.cells = make([]cell, width*height)
	for i := range c.cells {
		c.cells[i] = cell{text: " "}
	}
	return c
}

func (c *canvas) set(x, y int, text string, style textStyle) {
	c.setClip(x, y, c.w, text, style)
}

func (c *canvas) setClip(x, y, maxRight int, text string, style textStyle) {
	if y < 0 || y >= c.h {
		return
	}
	limit := min(maxRight, c.w)
	for _, r := range text {
		if x < 0 {
			x++
			continue
		}
		if x >= limit {
			return
		}
		c.cells[y*c.w+x] = cell{text: string(r), style: style}
		x++
	}
}

func (c *canvas) fill(x, y, w, h int, style textStyle) {
	for row := y; row < y+h; row++ {
		for col := x; col < x+w; col++ {
			if col >= 0 && col < c.w && row >= 0 && row < c.h {
				c.cells[row*c.w+col] = cell{text: " ", style: style}
			}
		}
	}
}

func (c *canvas) box(x, y, w, h int, title string, titleStyle, borderStyle textStyle) {
	if w < 2 || h < 2 {
		return
	}
	c.fill(x, y, w, h, textStyle{})
	for col := x + 1; col < x+w-1; col++ {
		c.set(col, y, "─", borderStyle)
		c.set(col, y+h-1, "─", borderStyle)
	}
	for row := y + 1; row < y+h-1; row++ {
		c.set(x, row, "│", borderStyle)
		c.set(x+w-1, row, "│", borderStyle)
	}
	c.set(x, y, "┌", borderStyle)
	c.set(x+w-1, y, "┐", borderStyle)
	c.set(x, y+h-1, "└", borderStyle)
	c.set(x+w-1, y+h-1, "┘", borderStyle)
	if title != "" && w > 2 {
		c.set(x+1, y, truncate(title, w-2), titleStyle)
	}
}

func (c *canvas) styleOf(style textStyle) lipgloss.Style {
	if cached, ok := c.styles[style]; ok {
		return cached
	}
	value := lipgloss.NewStyle()
	if style.fg != "" {
		value = value.Foreground(style.fg)
	}
	if style.bg != "" {
		value = value.Background(style.bg)
	}
	if style.bold {
		value = value.Bold(true)
	}
	c.styles[style] = value
	return value
}

// tintBorder restyles only the background of the outermost cells, so the
// quota-recovery pulse never replaces the frame's glyphs.
func (c *canvas) tintBorder(style textStyle) {
	if c.w == 0 || c.h == 0 {
		return
	}
	tint := func(x, y int) {
		index := y*c.w + x
		c.cells[index].style.bg = style.bg
	}
	for x := 0; x < c.w; x++ {
		tint(x, 0)
		tint(x, c.h-1)
	}
	for y := 0; y < c.h; y++ {
		tint(0, y)
		tint(c.w-1, y)
	}
}

func (c *canvas) lines() []string {
	lines := make([]string, c.h)
	for y := 0; y < c.h; y++ {
		var b strings.Builder
		start := 0
		current := c.cells[y*c.w].style
		for x := 1; x <= c.w; x++ {
			if x < c.w && c.cells[y*c.w+x].style == current {
				continue
			}
			var text strings.Builder
			for i := start; i < x; i++ {
				text.WriteString(c.cells[y*c.w+i].text)
			}
			value := text.String()
			if current == (textStyle{}) {
				b.WriteString(value)
			} else {
				b.WriteString(c.styleOf(current).Render(value))
			}
			if x < c.w {
				start = x
				current = c.cells[y*c.w+x].style
			}
		}
		lines[y] = b.String()
	}
	return lines
}

func (c *canvas) render() string { return strings.Join(c.lines(), "\n") }

// rect is a screen rectangle.
type rect struct {
	x, y, w, h int
}

func (r rect) inner() rect {
	return rect{x: r.x + 1, y: r.y + 1, w: max(r.w-2, 0), h: max(r.h-2, 0)}
}

func (r rect) right() int  { return r.x + r.w }
func (r rect) bottom() int { return r.y + r.h }

func splitH(area rect, count int) []rect {
	if count <= 0 {
		return nil
	}
	out := make([]rect, count)
	for i := 0; i < count; i++ {
		x0 := area.x + area.w*i/count
		x1 := area.x + area.w*(i+1)/count
		out[i] = rect{x: x0, y: area.y, w: x1 - x0, h: area.h}
	}
	return out
}

func splitHFixed(area rect, widths ...int) []rect {
	out := make([]rect, 0, len(widths))
	x := area.x
	for i, width := range widths {
		if i == len(widths)-1 {
			width = area.right() - x
		}
		width = max(width, 0)
		if x+width > area.right() {
			width = area.right() - x
		}
		out = append(out, rect{x: x, y: area.y, w: width, h: area.h})
		x += width
	}
	return out
}

func clampInt(value, low, high int) int {
	if value < low {
		return low
	}
	if value > high {
		return high
	}
	return value
}

func popup(area rect, width, height int) rect {
	width = min(width, max(area.w-2, 0))
	height = min(height, max(area.h-2, 0))
	return rect{
		x: area.x + (area.w-width)/2,
		y: area.y + (area.h-height)/2,
		w: width,
		h: height,
	}
}
