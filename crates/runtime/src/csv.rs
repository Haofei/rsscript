use std::fmt;
use std::str::Utf8Error;

#[derive(Debug, Clone)]
pub struct RowBuffer {
    bytes: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct Row {
    fields: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CsvError {
    message: String,
}

impl CsvError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for CsvError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

impl std::error::Error for CsvError {}

impl From<Utf8Error> for CsvError {
    fn from(error: Utf8Error) -> Self {
        Self::new(error.to_string())
    }
}

pub fn row_buffer_new(size: i64) -> RowBuffer {
    RowBuffer {
        bytes: Vec::with_capacity(crate::resource_budget::bounded_allocation_size(
            size,
            "CSV row buffer allocation",
        )),
    }
}

pub fn csv_parse_row(buffer: &RowBuffer) -> Result<Row, CsvError> {
    let text = std::str::from_utf8(&buffer.bytes)?;
    let Some(line) = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .nth(1)
        .or_else(|| text.lines().map(str::trim).find(|line| !line.is_empty()))
    else {
        return Err(CsvError::new("CSV buffer is empty"));
    };
    Ok(parse_csv_row_line(line))
}

fn parse_csv_row_line(line: &str) -> Row {
    Row {
        fields: line
            .split(',')
            .map(|field| field.trim().to_string())
            .collect(),
    }
}

pub fn row_field_string(row: &Row, index: i64) -> Result<String, CsvError> {
    let index = usize::try_from(index).map_err(|_| CsvError::new("negative CSV field index"))?;
    row.fields
        .get(index)
        .cloned()
        .ok_or_else(|| CsvError::new(format!("CSV field index `{index}` is out of bounds")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rows_without_filesystem_access() {
        let mut buffer = row_buffer_new(32);
        buffer.bytes.extend_from_slice(b"name,count\nrss,3\n");

        let row = csv_parse_row(&buffer).expect("row");
        assert_eq!(row_field_string(&row, 0).expect("name"), "rss");
        assert_eq!(row_field_string(&row, 1).expect("count"), "3");
    }

    #[test]
    fn rejects_invalid_input_and_indexes() {
        let buffer = row_buffer_new(0);
        assert_eq!(
            csv_parse_row(&buffer).expect_err("empty CSV").to_string(),
            "CSV buffer is empty"
        );
        let row = parse_csv_row_line("rss");
        assert_eq!(
            row_field_string(&row, -1)
                .expect_err("negative index")
                .to_string(),
            "negative CSV field index"
        );
    }
}
