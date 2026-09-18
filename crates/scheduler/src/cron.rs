/*
 * Licensed to the Apache Software Foundation (ASF) under one
 * or more contributor license agreements.  See the NOTICE file
 * distributed with this work for additional information
 * regarding copyright ownership.  The ASF licenses this file
 * to you under the Apache License, Version 2.0 (the
 * "License"); you may not use this file except in compliance
 * with the License.  You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing,
 * software distributed under the License is distributed on an
 * "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
 * KIND, either express or implied.  See the License for the
 * specific language governing permissions and limitations
 * under the License.
 */

use crate::{Error, invalid};
use jiff::{Timestamp, ToSpan, civil::Date, tz::TimeZone};

/// The product's five numeric cron fields: lists, ranges, steps, Sunday 0 or 7.
/// Bits avoid allocating sets while evaluating recurring plans.
pub(crate) struct Cron {
    minute: Field,
    hour: Field,
    day: Field,
    month: Field,
    weekday: Field,
}
struct Field {
    bits: u64,
    wildcard: bool,
}
impl Field {
    fn parse(input: &str, min: u8, max: u8, sunday: bool) -> Result<Self, Error> {
        let mut result = Self {
            bits: 0,
            wildcard: false,
        };
        let integer = |text: &str, low: u8, high: u8| {
            if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(invalid("cron fields must be numeric"));
            }
            text.parse::<u8>()
                .ok()
                .filter(|value| (low..=high).contains(value))
                .ok_or_else(|| invalid("cron field is out of range"))
        };
        for item in input.split(',') {
            let (base, step) = match item.split_once('/') {
                Some((base, step)) => (base, Some(integer(step, 1, max - min + 1)?)),
                None => (item, None),
            };
            let (start, end) = if base == "*" {
                result.wildcard = true;
                (min, max)
            } else if let Some((start, end)) = base.split_once('-') {
                (integer(start, min, max)?, integer(end, min, max)?)
            } else {
                let start = integer(base, min, max)?;
                (start, if step.is_some() { max } else { start })
            };
            if start > end {
                return Err(invalid("cron range is reversed"));
            }
            for value in (start..=end).step_by(usize::from(step.unwrap_or(1))) {
                let value = if sunday && value == 7 { 0 } else { value };
                result.bits |= 1 << value;
            }
        }
        Ok(result)
    }
    fn contains(&self, value: i8) -> bool {
        self.bits & (1 << value) != 0
    }
    fn values(&self) -> impl Iterator<Item = i8> + '_ {
        (0..60).filter(|value| self.contains(*value))
    }
}
impl Cron {
    pub fn parse(expression: &str) -> Result<Self, Error> {
        if expression.len() > 80 {
            return Err(invalid("cron expression exceeds 80 bytes"));
        }
        let fields: Vec<_> = expression.split_whitespace().collect();
        let [minute, hour, day, month, weekday] = fields.as_slice() else {
            return Err(invalid("cron requires five fields"));
        };
        let cron = Self {
            minute: Field::parse(minute, 0, 59, false)?,
            hour: Field::parse(hour, 0, 23, false)?,
            day: Field::parse(day, 1, 31, false)?,
            month: Field::parse(month, 1, 12, false)?,
            weekday: Field::parse(weekday, 0, 7, true)?,
        };
        if cron.weekday.wildcard
            && !cron.day.wildcard
            && !cron.month.values().any(|month| {
                let max = [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31][month as usize - 1];
                cron.day.values().any(|day| day <= max)
            })
        {
            return Err(invalid("cron has no possible calendar date"));
        }
        Ok(cron)
    }
    fn matches_date(&self, date: Date) -> bool {
        if !self.month.contains(date.month()) {
            return false;
        }
        let day = self.day.contains(date.day());
        let weekday = self
            .weekday
            .contains(date.weekday().to_sunday_zero_offset());
        if !self.day.wildcard && !self.weekday.wildcard {
            day || weekday
        } else {
            day && weekday
        }
    }
    pub fn next(
        &self,
        after: i64,
        not_before: i64,
        through: i64,
        zone: &TimeZone,
    ) -> Result<Option<i64>, Error> {
        let lower = (after + 1).max(not_before);
        if lower > through {
            return Ok(None);
        }
        let mut date = Timestamp::from_millisecond(lower)?
            .to_zoned(zone.clone())
            .date();
        let last = Timestamp::from_millisecond(through)?
            .to_zoned(zone.clone())
            .date();
        while date <= last {
            if self.matches_date(date) {
                let mut next = None;
                for hour in self.hour.values() {
                    for minute in self.minute.values() {
                        let local = date.at(hour, minute, 0, 0);
                        let ambiguous = zone.to_ambiguous_zoned(local);
                        // Gaps have no matching wall time. Folds have two real
                        // occurrences: select by timestamp, not wall-clock order.
                        for resolved in [ambiguous.clone().earlier()?, ambiguous.later()?] {
                            let at = resolved.timestamp().as_millisecond();
                            if resolved.datetime() == local && at >= lower && at <= through {
                                next = Some(next.map_or(at, |previous: i64| previous.min(at)));
                            }
                        }
                    }
                }
                if next.is_some() {
                    return Ok(next);
                }
            }
            date = date.checked_add(1.day())?;
        }
        Ok(None)
    }
}
