//! Local time without a date/time dependency.

pub struct LocalTime {
    pub year: i32,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    /// Seconds east of UTC (PST = -28800).
    pub utc_offset_secs: i32,
}

pub fn local_now() -> LocalTime {
    unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        LocalTime {
            year: tm.tm_year + 1900,
            month: (tm.tm_mon + 1) as u8,
            day: tm.tm_mday as u8,
            hour: tm.tm_hour as u8,
            minute: tm.tm_min as u8,
            second: tm.tm_sec as u8,
            utc_offset_secs: tm.tm_gmtoff as i32,
        }
    }
}

impl LocalTime {
    pub fn date(&self) -> String {
        format!("{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }

    pub fn stamp(&self) -> String {
        format!(
            "{:04}-{:02}-{:02} {:02}.{:02}.{:02}",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        )
    }

    /// PDF date string, e.g. `D:20261101171037-08'00'`.
    pub fn pdf_date(&self) -> String {
        let off = self.utc_offset_secs;
        let sign = if off < 0 { '-' } else { '+' };
        let off = off.abs();
        format!(
            "D:{:04}{:02}{:02}{:02}{:02}{:02}{}{:02}'{:02}'",
            self.year,
            self.month,
            self.day,
            self.hour,
            self.minute,
            self.second,
            sign,
            off / 3600,
            (off % 3600) / 60
        )
    }
}
