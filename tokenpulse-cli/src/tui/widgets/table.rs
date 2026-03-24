use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style, Stylize},
    widgets::Widget,
};

pub struct StyledTable<'a> {
    headers: Vec<&'a str>,
    rows: Vec<Vec<String>>,
    widths: Vec<u16>,
    header_color: Color,
    row_colors: (Color, Color),
}

impl<'a> StyledTable<'a> {
    pub fn new(headers: Vec<&'a str>) -> Self {
        let widths = vec![20; headers.len()];
        Self {
            headers,
            rows: Vec::new(),
            widths,
            header_color: Color::Cyan,
            row_colors: (Color::Reset, Color::DarkGray),
        }
    }

    pub fn row(mut self, row: Vec<String>) -> Self {
        self.rows.push(row);
        self
    }

    pub fn widths(mut self, widths: Vec<u16>) -> Self {
        self.widths = widths;
        self
    }

    pub fn header_color(mut self, color: Color) -> Self {
        self.header_color = color;
        self
    }
}

impl<'a> Widget for StyledTable<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height < 2 {
            return;
        }

        let mut y = area.y;

        let header_style = Style::default().fg(self.header_color).bold();
        let mut x = area.x;
        for (i, header) in self.headers.iter().enumerate() {
            let width = self.widths.get(i).copied().unwrap_or(20);
            buf.set_string(x, y, *header, header_style);
            x += width;
        }
        y += 1;

        for (row_idx, row) in self.rows.iter().enumerate() {
            if y >= area.y + area.height {
                break;
            }

            let color = if row_idx % 2 == 0 {
                self.row_colors.0
            } else {
                self.row_colors.1
            };
            let style = Style::default().fg(color);

            let mut x = area.x;
            for (i, cell) in row.iter().enumerate() {
                let width = self.widths.get(i).copied().unwrap_or(20);
                let truncated: String = cell
                    .chars()
                    .take(width.saturating_sub(1) as usize)
                    .collect();
                buf.set_string(x, y, &truncated, style);
                x += width;
            }
            y += 1;
        }
    }
}
