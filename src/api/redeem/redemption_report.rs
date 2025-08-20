use std::ops::Add;

use itertools::Itertools;
use rand::distr::Distribution;
use rand::distr::StandardUniform;
use rand::rng;
use rand::Rng;
use serde::Deserialize;
use serde::Serialize;
use strum::Display;
use strum::EnumString;
use strum::VariantNames;
use tasm_lib::triton_vm::prelude::BFieldElement;

use crate::api::export::NativeCurrencyAmount;
use crate::api::export::Network;
use crate::api::export::Timestamp;
use crate::models::state::wallet::address::generation_address::GenerationReceivingAddress;

#[derive(
    Debug, Clone, Copy, Default, EnumString, VariantNames, Display, Serialize, Deserialize,
)]
#[strum(serialize_all = "kebab_case", ascii_case_insensitive)]
pub enum RedemptionReportDisplayFormat {
    #[default]
    Readable,
    Detailed,
    ColonSeparated,
    ColonSeparatedTestnet,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RedemptionReportEntry {
    amount: NativeCurrencyAmount,
    earliest_release_date: Option<Timestamp>,
    address: GenerationReceivingAddress,
}

impl RedemptionReportEntry {
    fn new(
        amount: NativeCurrencyAmount,
        earliest_release_date: Option<Timestamp>,
        address: GenerationReceivingAddress,
    ) -> Self {
        Self {
            amount,
            earliest_release_date,
            address,
        }
    }
    fn headings() -> [String; 3] {
        ["amount", "earliest release date", "address"].map(|s| s.to_string())
    }

    fn column_widths(format: RedemptionReportDisplayFormat) -> [usize; 3] {
        let amount_width = usize::max(
            NativeCurrencyAmount::max().display_lossless().len(),
            NativeCurrencyAmount::max().to_nau().to_string().len(),
        );
        let release_date_width = match format {
            RedemptionReportDisplayFormat::Readable => Timestamp::now().standard_format().len(),
            RedemptionReportDisplayFormat::Detailed => BFieldElement::MAX.to_string().len(),
            RedemptionReportDisplayFormat::ColonSeparated
            | RedemptionReportDisplayFormat::ColonSeparatedTestnet => {
                BFieldElement::MAX.to_string().len()
            }
        };
        let random_address = GenerationReceivingAddress::derive_from_seed(rng().random());
        let network = Network::Main;
        let address_width = match format {
            RedemptionReportDisplayFormat::Readable => random_address
                .to_bech32m_abbreviated(network)
                .unwrap()
                .len(),
            RedemptionReportDisplayFormat::Detailed => {
                random_address.to_bech32m(network).unwrap().len()
            }
            RedemptionReportDisplayFormat::ColonSeparated
            | RedemptionReportDisplayFormat::ColonSeparatedTestnet => {
                random_address.to_bech32m(network).unwrap().len()
            }
        };

        let entry_widths = [amount_width, release_date_width, address_width];
        let heading_widths = Self::headings().map(|h| h.len());
        heading_widths
            .into_iter()
            .zip(entry_widths)
            .map(|(l, r)| usize::max(l, r))
            .collect_vec()
            .try_into()
            .unwrap()
    }

    fn render(&self, format: RedemptionReportDisplayFormat, column_widths: [usize; 3]) -> String {
        let amount = match format {
            RedemptionReportDisplayFormat::Readable => self.amount.display_lossless(),
            RedemptionReportDisplayFormat::Detailed => self.amount.to_nau().to_string(),
            RedemptionReportDisplayFormat::ColonSeparated
            | RedemptionReportDisplayFormat::ColonSeparatedTestnet => {
                self.amount.display_lossless()
            }
        };

        let amount_padded = format!("{:>width$}", amount, width = column_widths[0]);

        let earliest_release_date = match (format, self.earliest_release_date) {
            (_, None) => "-".to_string(),
            (RedemptionReportDisplayFormat::Readable, Some(rd)) => rd.standard_format(),
            (RedemptionReportDisplayFormat::Detailed, Some(rd)) => rd.to_millis().to_string(),
            (
                RedemptionReportDisplayFormat::ColonSeparated
                | RedemptionReportDisplayFormat::ColonSeparatedTestnet,
                Some(rd),
            ) => rd.to_millis().to_string(),
        };
        let earliest_release_date_padded =
            format!("{:width$}", earliest_release_date, width = column_widths[1]);

        let network = Network::Main;
        let address = match format {
            RedemptionReportDisplayFormat::Readable => {
                self.address.to_bech32m_abbreviated(network).unwrap()
            }
            RedemptionReportDisplayFormat::Detailed => self.address.to_bech32m(network).unwrap(),
            RedemptionReportDisplayFormat::ColonSeparated => {
                self.address.to_bech32m(network).unwrap()
            }
            RedemptionReportDisplayFormat::ColonSeparatedTestnet => {
                self.address.to_bech32m(Network::Testnet).unwrap()
            }
        };
        let address_padded = format!("{:width$}", address, width = column_widths[2]);

        match format {
            RedemptionReportDisplayFormat::Readable => {
                format!(
                    "| {} | {} | {} |\n",
                    amount_padded, earliest_release_date_padded, address_padded
                )
            }
            RedemptionReportDisplayFormat::Detailed => {
                format!(
                    "{} {} {}\n",
                    amount_padded, earliest_release_date_padded, address_padded
                )
            }
            RedemptionReportDisplayFormat::ColonSeparated
            | RedemptionReportDisplayFormat::ColonSeparatedTestnet => {
                if let Some(release_date) = self.earliest_release_date {
                    format!("{}:{}:{}\n", address, amount, release_date.to_millis())
                } else {
                    format!("{}:{}\n", address, amount)
                }
            }
        }
    }
}

impl Distribution<RedemptionReportEntry> for StandardUniform {
    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> RedemptionReportEntry {
        let amount = NativeCurrencyAmount::from_nau(
            rng.random_range(0_i128..NativeCurrencyAmount::max().to_nau()) >> 5,
        );
        let earliest_release_date = if rng.random_bool(0.5_f64) {
            None
        } else {
            Some(
                Timestamp(BFieldElement::new(
                    rng.random_range(0_u64..(1000 * 60 * 60 * 24 * 365)),
                )) + Network::Main.launch_date(),
            )
        };
        let address = GenerationReceivingAddress::derive_from_seed(rng.random());

        RedemptionReportEntry {
            amount,
            earliest_release_date,
            address,
        }
    }
}

impl Add<&RedemptionReportEntry> for &RedemptionReportEntry {
    type Output = RedemptionReportEntry;

    fn add(self, rhs: &RedemptionReportEntry) -> Self::Output {
        let earliest_release_date = self
            .earliest_release_date
            .into_iter()
            .chain(rhs.earliest_release_date)
            .min();
        let amount = self.amount + rhs.amount;
        let address = self.address;
        RedemptionReportEntry {
            amount,
            earliest_release_date,
            address,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct RedemptionReport {
    table: Vec<RedemptionReportEntry>,
}

impl RedemptionReport {
    pub fn new() -> Self {
        Self { table: vec![] }
    }

    /// Compresses all rows that have the same destination address and timelock,
    /// ignoring the concrete release date.
    pub fn compressed(self) -> Self {
        let mut new_table: Vec<RedemptionReportEntry> = vec![];
        for old_row in self.table {
            let mut have_match = false;
            'inner: for new_row in &mut new_table {
                if old_row.address != new_row.address {
                    continue;
                }
                match (new_row.earliest_release_date, old_row.earliest_release_date) {
                    (None, Some(_)) | (Some(_), None) => (),
                    _ => {
                        *new_row = &old_row + new_row;
                        have_match = true;
                        break 'inner;
                    }
                }
            }

            if !have_match {
                new_table.push(old_row);
            }
        }
        Self { table: new_table }
    }

    pub fn add_row(
        &mut self,
        amount: NativeCurrencyAmount,
        release_date: Option<Timestamp>,
        address: GenerationReceivingAddress,
    ) {
        self.table
            .push(RedemptionReportEntry::new(amount, release_date, address));
    }

    fn render_header(format: RedemptionReportDisplayFormat) -> String {
        let headings = RedemptionReportEntry::headings();
        let column_widths = RedemptionReportEntry::column_widths(format);
        match format {
            RedemptionReportDisplayFormat::Readable => {
                let total_width = column_widths.into_iter().sum::<usize>()
                    + "| ".len()
                    + 2 * " | ".len()
                    + " |".len();
                format!(
                    "{:-<widtha$}\n| {:width0$} | {:width1$} | {:width2$} |\n{:-<widthb$}\n",
                    "",
                    headings[0],
                    headings[1],
                    headings[2],
                    "",
                    widtha = total_width,
                    width0 = column_widths[0],
                    width1 = column_widths[1],
                    width2 = column_widths[2],
                    widthb = total_width
                )
            }
            RedemptionReportDisplayFormat::Detailed => format!(
                "{:width0$} {:width1$} {:width2$}\n",
                headings[0],
                headings[1],
                headings[2],
                width0 = column_widths[0],
                width1 = column_widths[1],
                width2 = column_widths[2],
            ),
            RedemptionReportDisplayFormat::ColonSeparated
            | RedemptionReportDisplayFormat::ColonSeparatedTestnet => "".to_string(),
        }
    }

    fn render_body(&self, format: RedemptionReportDisplayFormat) -> String {
        let column_widths = RedemptionReportEntry::column_widths(format);
        let mut s = "".to_string();
        for row in &self.table {
            s = format!("{s}{}", row.render(format, column_widths));
        }
        s
    }

    fn render_footer(&self, format: RedemptionReportDisplayFormat) -> String {
        let column_widths = RedemptionReportEntry::column_widths(format);
        match format {
            RedemptionReportDisplayFormat::Readable => {
                let total_amount = self
                    .table
                    .iter()
                    .map(|tr| tr.amount)
                    .sum::<NativeCurrencyAmount>();
                let total_width = column_widths.into_iter().sum::<usize>()
                    + "| ".len()
                    + 2 * " | ".len()
                    + " |".len();
                format!(
                    "{:-<width$}\n| total: {}",
                    "",
                    total_amount.display_lossless(),
                    width = total_width,
                )
            }
            RedemptionReportDisplayFormat::Detailed => "".to_string(),
            RedemptionReportDisplayFormat::ColonSeparated
            | RedemptionReportDisplayFormat::ColonSeparatedTestnet => "".to_string(),
        }
    }

    fn render_table(&self, format: RedemptionReportDisplayFormat) -> String {
        format!(
            "{}{}{}",
            Self::render_header(format),
            self.render_body(format),
            self.render_footer(format)
        )
    }

    pub fn render(&self, format: RedemptionReportDisplayFormat) -> String {
        format!("{}\n", self.render_table(format))
    }
}

impl Distribution<RedemptionReport> for StandardUniform {
    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> RedemptionReport {
        let num_rows = rng.random_range(0..25);
        let table = (0..num_rows)
            .map(|_| rng.random::<RedemptionReportEntry>())
            .collect_vec();

        RedemptionReport { table }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn can_render_random_report() {
        let mut rng = rng();
        let report = rng.random::<RedemptionReport>();
        let format = match rng.random_range(0..4) {
            0 => RedemptionReportDisplayFormat::Readable,
            1 => RedemptionReportDisplayFormat::Detailed,
            2 => RedemptionReportDisplayFormat::ColonSeparated,
            3 => RedemptionReportDisplayFormat::ColonSeparatedTestnet,
            _ => panic!("rng error"),
        };
        println!("{}", report.render(format)); // no crash
    }
}
