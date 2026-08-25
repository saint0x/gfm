#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VirtualSurface {
    IconGrid,
    ListRows,
    ColumnRows,
    GalleryFilmstrip,
    SearchResults,
    TrashRows,
}

impl VirtualSurface {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::IconGrid => "icon-grid",
            Self::ListRows => "list-rows",
            Self::ColumnRows => "column-rows",
            Self::GalleryFilmstrip => "gallery-filmstrip",
            Self::SearchResults => "search-results",
            Self::TrashRows => "trash-rows",
        }
    }

    pub const fn unit(self) -> &'static str {
        match self {
            Self::IconGrid => "cell",
            Self::GalleryFilmstrip => "item",
            Self::ListRows | Self::ColumnRows | Self::SearchResults | Self::TrashRows => "row",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VirtualWindow {
    pub start: usize,
    pub end: usize,
    pub capacity: usize,
    pub total: usize,
}

impl VirtualWindow {
    pub fn rows(total: usize, scroll_row: u32, viewport_rows: u16) -> Self {
        Self::new(
            total,
            scroll_row as usize,
            usize::from(viewport_rows.max(1)),
        )
    }

    pub fn items(total: usize, scroll_item: u32, viewport_items: u16) -> Self {
        Self::new(
            total,
            scroll_item as usize,
            usize::from(viewport_items.max(1)),
        )
    }

    pub fn grid(total: usize, scroll_row: u16, viewport_rows: u16, columns: u16) -> Self {
        let columns = usize::from(columns.max(1));
        let offset = usize::from(scroll_row).saturating_mul(columns);
        let capacity = usize::from(viewport_rows.max(1)).saturating_mul(columns);
        Self::new(total, offset, capacity)
    }

    pub fn new(total: usize, requested_start: usize, capacity: usize) -> Self {
        let capacity = capacity.max(1);
        let start = requested_start.min(total);
        let end = start.saturating_add(capacity).min(total);
        Self {
            start,
            end,
            capacity,
            total,
        }
    }

    pub const fn rendered(self) -> usize {
        self.end.saturating_sub(self.start)
    }

    pub const fn bounded(self) -> bool {
        self.rendered() <= self.capacity
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VirtualizationContract {
    pub surface: VirtualSurface,
    pub total: usize,
    pub visible_start: usize,
    pub visible_end: usize,
    pub rendered: usize,
    pub render_capacity: usize,
    pub bounded: bool,
}

impl VirtualizationContract {
    pub fn new(surface: VirtualSurface, window: VirtualWindow) -> Self {
        Self {
            surface,
            total: window.total,
            visible_start: window.start,
            visible_end: window.end,
            rendered: window.rendered(),
            render_capacity: window.capacity,
            bounded: window.bounded(),
        }
    }

    pub fn rows(
        surface: VirtualSurface,
        total: usize,
        scroll_row: u32,
        viewport_rows: u16,
    ) -> Self {
        Self::new(
            surface,
            VirtualWindow::rows(total, scroll_row, viewport_rows),
        )
    }

    pub fn items(
        surface: VirtualSurface,
        total: usize,
        scroll_item: u32,
        viewport_items: u16,
    ) -> Self {
        Self::new(
            surface,
            VirtualWindow::items(total, scroll_item, viewport_items),
        )
    }

    pub fn grid(total: usize, scroll_row: u16, viewport_rows: u16, columns: u16) -> Self {
        Self::new(
            VirtualSurface::IconGrid,
            VirtualWindow::grid(total, scroll_row, viewport_rows, columns),
        )
    }

    pub fn as_tsv(&self) -> String {
        format!(
            "virtualization\t{}\tunit={}\ttotal={}\tvisible={}..{}\trendered={}\tcapacity={}\tbounded={}",
            self.surface.as_str(),
            self.surface.unit(),
            self.total,
            self.visible_start,
            self.visible_end,
            self.rendered,
            self.render_capacity,
            self.bounded
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_windows_are_bounded_for_huge_directories() {
        let contract = VirtualizationContract::rows(VirtualSurface::ListRows, 250_000, 199_990, 32);

        assert_eq!(contract.visible_start, 199_990);
        assert_eq!(contract.visible_end, 200_022);
        assert_eq!(contract.rendered, 32);
        assert!(contract.bounded);
    }

    #[test]
    fn grid_windows_bound_cells_by_columns_and_viewport_rows() {
        let contract = VirtualizationContract::grid(400_000, 40_000, 4, 6);

        assert_eq!(contract.visible_start, 240_000);
        assert_eq!(contract.visible_end, 240_024);
        assert_eq!(contract.render_capacity, 24);
        assert_eq!(contract.rendered, 24);
        assert!(contract.bounded);
    }

    #[test]
    fn windows_clamp_overscroll_without_rendering_padding() {
        let contract =
            VirtualizationContract::rows(VirtualSurface::TrashRows, 100_000, 200_000, 24);

        assert_eq!(contract.visible_start, 100_000);
        assert_eq!(contract.visible_end, 100_000);
        assert_eq!(contract.rendered, 0);
        assert!(contract.bounded);
    }

    #[test]
    fn tsv_output_is_stable_for_cli_and_fozzy() {
        let contract =
            VirtualizationContract::rows(VirtualSurface::SearchResults, 100_000, 99_990, 12);

        assert_eq!(
            contract.as_tsv(),
            "virtualization\tsearch-results\tunit=row\ttotal=100000\tvisible=99990..100000\trendered=10\tcapacity=12\tbounded=true"
        );
    }
}
