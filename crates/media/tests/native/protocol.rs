#[derive(Debug, PartialEq)]
pub struct Geometry {
    pub window: u32,
    pub display: u32,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub display_width: u32,
    pub display_height: u32,
}

impl Geometry {
    pub fn parse(reply: &str) -> Result<Self, &'static str> {
        let fields: Vec<_> = reply.split_whitespace().collect();
        if fields.len() != 9 || fields[0] != "ready" {
            return Err("invalid fixture geometry");
        }
        let numbers: Vec<u32> = fields[1..]
            .iter()
            .map(|value| value.parse())
            .collect::<Result<_, _>>()
            .map_err(|_| "invalid number")?;
        let value = Self {
            window: numbers[0],
            display: numbers[1],
            x: numbers[2],
            y: numbers[3],
            width: numbers[4],
            height: numbers[5],
            display_width: numbers[6],
            display_height: numbers[7],
        };
        if value.window == 0
            || value.display == 0
            || value.width == 0
            || value.height == 0
            || value
                .x
                .checked_add(value.width)
                .is_none_or(|right| right > value.display_width)
            || value
                .y
                .checked_add(value.height)
                .is_none_or(|bottom| bottom > value.display_height)
        {
            return Err("fixture does not fit its display");
        }
        Ok(value)
    }
}

pub fn acknowledged(reply: &str, marker: u32) -> bool {
    reply.trim() == format!("pattern {marker}")
}
