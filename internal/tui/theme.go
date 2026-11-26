package tui

import "github.com/charmbracelet/lipgloss"

type styles struct {
	title    lipgloss.Style
	active   lipgloss.Style
	dim      lipgloss.Style
	text     lipgloss.Style
	selected lipgloss.Style
	good     lipgloss.Style
	warn     lipgloss.Style
	danger   lipgloss.Style
	error    lipgloss.Style
	toast    lipgloss.Style
	cost     lipgloss.Style
	badgeOn  lipgloss.Style
	badgeOff lipgloss.Style
	rule     lipgloss.Style
	heat     [6]lipgloss.Style

	companyAnthropic lipgloss.Style
	companyOpenAI    lipgloss.Style
	companyGoogle    lipgloss.Style
}

func stylesFor(theme int) styles {
	dark := theme != 2
	fg := func(darkValue, lightValue lipgloss.Color) lipgloss.Color {
		if dark {
			return darkValue
		}
		return lightValue
	}
	text := lipgloss.NewStyle().Foreground(fg("252", "236"))
	dim := lipgloss.NewStyle().Foreground(fg("244", "245"))
	heat := [6]lipgloss.Style{
		lipgloss.NewStyle().Foreground(fg("238", "250")),
		lipgloss.NewStyle().Foreground(fg("22", "151")),
		lipgloss.NewStyle().Foreground(fg("28", "34")),
		lipgloss.NewStyle().Foreground(fg("34", "28")),
		lipgloss.NewStyle().Foreground(fg("40", "22")),
		lipgloss.NewStyle().Foreground(fg("46", "16")),
	}
	return styles{
		title:            lipgloss.NewStyle().Bold(true).Foreground(fg("231", "232")),
		active:           lipgloss.NewStyle().Bold(true).Foreground(fg("16", "231")).Background(fg("81", "33")),
		dim:              dim,
		text:             text,
		selected:         lipgloss.NewStyle().Foreground(fg("16", "231")).Background(fg("240", "153")),
		good:             lipgloss.NewStyle().Foreground(fg("42", "28")),
		warn:             lipgloss.NewStyle().Foreground(fg("214", "130")),
		danger:           lipgloss.NewStyle().Foreground(fg("203", "160")),
		error:            lipgloss.NewStyle().Foreground(fg("203", "160")).Bold(true),
		toast:            lipgloss.NewStyle().Bold(true).Foreground(fg("16", "231")).Background(fg("42", "34")),
		cost:             lipgloss.NewStyle().Bold(true).Foreground(fg("220", "136")),
		badgeOn:          lipgloss.NewStyle().Bold(true).Foreground(fg("42", "28")),
		badgeOff:         lipgloss.NewStyle().Foreground(fg("244", "245")),
		rule:             lipgloss.NewStyle().Foreground(fg("238", "251")),
		heat:             heat,
		companyAnthropic: lipgloss.NewStyle().Foreground(fg("209", "166")),
		companyOpenAI:    lipgloss.NewStyle().Foreground(fg("114", "28")),
		companyGoogle:    lipgloss.NewStyle().Foreground(fg("75", "25")),
	}
}
