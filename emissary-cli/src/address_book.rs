// Permission is hereby granted, free of charge, to any person obtaining a
// copy of this software and associated documentation files (the "Software"),
// to deal in the Software without restriction, including without limitation
// the rights to use, copy, modify, merge, publish, distribute, sublicense,
// and/or sell copies of the Software, and to permit persons to whom the
// Software is furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in
// all copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS
// OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
// FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
// DEALINGS IN THE SOFTWARE.

use crate::config::AddressBookConfig;

use emissary_core::runtime::AddressBook;
use futures::channel::oneshot;
use reqwest::{
    header::{HeaderMap, HeaderValue, CONNECTION},
    Client, Proxy,
};
use tokio::{
    fs::File,
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
};

use std::{
    collections::HashMap, future::Future, path::PathBuf, pin::Pin, sync::Arc, time::Duration,
};

/// Logging target for the file
const LOG_TARGET: &str = "emissary::address-book";

/// Backoff if downloading the hosts file fails.
const RETRY_BACKOFF: Duration = Duration::from_secs(30);

/// How many times each subscription is tried before giving up.
const SUBSCRIPTION_NUM_RETRIES: usize = 5usize;

/// Address book.
pub struct AddressBookManager {
    /// Path to address book.
    address_book_path: &'static str,

    /// URL from which the primary `hosts.txt` is downloaded from.
    hosts_url: String,

    /// Additional subscriptions.
    subscriptions: Vec<String>,
}

impl AddressBookManager {
    /// Create new [`AddressBookManager`].
    pub fn new(base_path: PathBuf, config: AddressBookConfig) -> Self {
        Self {
            address_book_path: base_path
                .join("addressbook/addresses")
                .to_str()
                .expect("to succeed")
                .to_string()
                .leak(),
            hosts_url: config.default,
            subscriptions: config.subscriptions,
        }
    }

    /// Get opaque handling implementing [`AddressBook`].
    pub fn handle(&self) -> Arc<dyn AddressBook> {
        Arc::new(AddressBookHandle {
            address_book_path: self.address_book_path,
        })
    }

    /// Attempt to download `hosts.txt` from `url`.
    async fn download(client: &Client, url: &str) -> Option<String> {
        let response = match client
            .get(format!("{}", url))
            .headers(HeaderMap::from_iter([(
                CONNECTION,
                HeaderValue::from_static("close"),
            )]))
            .send()
            .await
        {
            Err(error) => {
                tracing::debug!(
                    target: LOG_TARGET,
                    ?url,
                    ?error,
                    "failed to fetch hosts.txt"
                );
                return None;
            }
            Ok(response) => response,
        };

        if !response.status().is_success() {
            tracing::debug!(
                target: LOG_TARGET,
                ?url,
                status = ?response.status(),
                "request to address book server failed",
            );
            return None;
        }

        match response.bytes().await {
            Ok(response) => match std::str::from_utf8(&response) {
                Ok(response) => Some(response.to_owned()),
                Err(error) => {
                    tracing::debug!(
                        target: LOG_TARGET,
                        ?url,
                        ?error,
                        "failed to convert `hosts.txt` to utf-8",
                    );
                    None
                }
            },
            Err(error) => {
                tracing::debug!(
                    target: LOG_TARGET,
                    ?url,
                    ?error,
                    "failed to get response from address book server"
                );
                return None;
            }
        }
    }

    /// Parse `hosts` into (key, value) tuple and merge it with `addresses`.
    ///
    /// Addresses already present in `addresses` will be ignored.
    async fn parse_and_merge(&self, addresses: &mut HashMap<String, String>, hosts: String) {
        for line in hosts.lines() {
            if let Some((key, value)) = line.split_once('=') {
                let key = key.trim().to_string();

                if addresses.contains_key(&key) {
                    tracing::trace!(
                        target: LOG_TARGET,
                        %key,
                        "skipping already-existing address",
                    );
                } else {
                    let value = match value.find("#!") {
                        Some(index) => value[..index].trim().to_string(),
                        None => value.trim().to_string(),
                    };

                    addresses.insert(key.trim().to_string(), value);
                }
            }
        }

        match File::create(&self.address_book_path).await {
            Err(error) => tracing::error!(
                target: LOG_TARGET,
                ?error,
                "failed to open address book",
            ),
            Ok(mut file) => {
                let address_book = addresses.iter().fold(Vec::new(), |mut out, (key, value)| {
                    out.extend_from_slice(format!("{key}={value}\n").as_bytes());
                    out
                });

                if let Err(error) = file.write_all(&address_book).await {
                    tracing::error!(
                        target: LOG_TARGET,
                        ?error,
                        "failed to write to address book",
                    );
                }
            }
        }
    }

    /// Start event loop for [`AddressBookManager`].
    ///
    /// Before the address book subscription download starts, [`AddressBook`] waits on
    /// `http_proxy_ready_rx` which the HTTP proxy sends a signal to once it's ready.
    pub async fn run(
        self,
        http_port: u16,
        http_host: String,
        http_proxy_ready_rx: oneshot::Receiver<()>,
    ) {
        if let Err(error) = http_proxy_ready_rx.await {
            tracing::error!(
                target: LOG_TARGET,
                ?error,
                "http proxy failed to start, cannot start address book",
            );
        }

        tracing::info!(
            target: LOG_TARGET,
            ?http_port,
            ?http_host,
            default = ?self.hosts_url,
            subscriptions = ?self.subscriptions,
            "create address book",
        );

        let client = Client::builder()
            .proxy(Proxy::http(format!("http://{http_host}:{http_port}")).expect("to succeed"))
            .http1_title_case_headers()
            .build()
            .expect("to succeed");

        let mut addresses = HashMap::<String, String>::new();

        loop {
            match Self::download(&client, &self.hosts_url).await {
                Some(hosts) => {
                    tracing::info!(
                        target: LOG_TARGET,
                        url = %self.hosts_url,
                        "hosts.txt downloaded",
                    );

                    self.parse_and_merge(&mut addresses, hosts).await;
                    break;
                }
                None => tokio::time::sleep(RETRY_BACKOFF).await,
            }
        }

        for subscription in &self.subscriptions {
            for _ in 0..SUBSCRIPTION_NUM_RETRIES {
                match Self::download(&client, &subscription).await {
                    Some(hosts) => {
                        tracing::info!(
                            target: LOG_TARGET,
                            url = subscription,
                            "hosts.txt downloaded",
                        );

                        self.parse_and_merge(&mut addresses, hosts).await;
                        break;
                    }
                    None => tokio::time::sleep(RETRY_BACKOFF).await,
                }
            }
        }
    }
}

/// Address book handle.
#[derive(Clone)]
pub struct AddressBookHandle {
    /// Path to address book.
    address_book_path: &'static str,
}

impl AddressBook for AddressBookHandle {
    fn resolve(&self, host: String) -> Pin<Box<dyn Future<Output = Option<String>> + Send>> {
        let path = self.address_book_path;

        Box::pin(async move {
            let file = tokio::fs::File::open(path).await.ok()?;
            let mut reader = BufReader::new(file).lines();

            while let Ok(line) = reader.next_line().await {
                if let Some((key, value)) = line?.split_once('=') {
                    if key.trim() == host {
                        return Some(value.trim().to_string());
                    }
                }
            }

            None
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn save_only_destination() {
        let dir = tempdir().unwrap();
        let address_book = AddressBookManager::new(
            dir.into_path(),
            AddressBookConfig {
                default: String::from("url"),
                subscriptions: Vec::new(),
            },
        );

        let mut addresses = HashMap::<String, String>::new();
        let hosts = "tracker2.postman.i2p=lnQ6yoBTxQuQU8EQ1FlF395ITIQF-HGJxUeFvzETLFnoczNjQvKDbtSB7aHhn853zjVXrJBgwlB9sO57KakBDaJ50lUZgVPhjlI19TgJ-CxyHhHSCeKx5JzURdEW-ucdONMynr-b2zwhsx8VQCJwCEkARvt21YkOyQDaB9IdV8aTAmP~PUJQxRwceaTMn96FcVenwdXqleE16fI8CVFOV18jbJKrhTOYpTtcZKV4l1wNYBDwKgwPx5c0kcrRzFyw5~bjuAKO~GJ5dR7BQsL7AwBoQUS4k1lwoYrG1kOIBeDD3XF8BWb6K3GOOoyjc1umYKpur3G~FxBuqtHAsDRICrsRuil8qK~whOvj8uNTv~ohZnTZHxTLgi~sDyo98BwJ-4Y4NMSuF4GLzcgLypcR1D1WY2tDqMKRYFVyLE~MTPVjRRgXfcKolykQ666~Go~A~~CNV4qc~zlO6F4bsUhVZDU7WJ7mxCAwqaMiJsL-NgIkb~SMHNxIzaE~oy0agHJMBQAEAAcAAA==#!oldsig=i02RMv3Hy86NGhVo2O3byIf6xXqWrzrRibSabe5dmNfRRQPZO9L25A==#date=1598641102#action=adddest#sig=cB-mY~sp1uuEmcQJqremV1D6EDWCe3IwPv4lBiGAXgKRYc5MLBBzYvJXtXmOawpfLKeNM~v5fWlXYsDfKf5nDA==#olddest=lnQ6yoBTxQuQU8EQ1FlF395ITIQF-HGJxUeFvzETLFnoczNjQvKDbtSB7aHhn853zjVXrJBgwlB9sO57KakBDaJ50lUZgVPhjlI19TgJ-CxyHhHSCeKx5JzURdEW-ucdONMynr-b2zwhsx8VQCJwCEkARvt21YkOyQDaB9IdV8aTAmP~PUJQxRwceaTMn96FcVenwdXqleE16fI8CVFOV18jbJKrhTOYpTtcZKV4l1wNYBDwKgwPx5c0kcrRzFyw5~bjuAKO~GJ5dR7BQsL7AwBoQUS4k1lwoYrG1kOIBeDD3XF8BWb6K3GOOoyjc1umYKpur3G~FxBuqtHAsDRICkEbKUqJ9mPYQlTSujhNxiRIW-oLwMtvayCFci99oX8MvazPS7~97x0Gsm-onEK1Td9nBdmq30OqDxpRtXBimbzkLbR1IKObbg9HvrKs3L-kSyGwTUmHG9rSQSoZEvFMA-S0EXO~o4g21q1oikmxPMhkeVwQ22VHB0-LZJfmLr4SAAAA\npsi.i2p=a11l91etedRW5Kl2GhdDI9qiRBbDRAQY6TWJb8KlSc0P9WUrEviABAAltqDU1DFJrRhMAZg5i6rWGszkJrF-pWLQK9JOH33l4~mQjB8Hkt83l9qnNJPUlGlh9yIfBY40CQ0Ermy8gzjHLayUpypDJFv2V6rHLwxAQeaXJu8YXbyvCucEu9i6HVO49akXW9YSxcZEqxK04wZnjBqhHGlVbehleMqTx9nkd0pUpBZz~vIaG9matUSHinopEo6Wegml9FEz~FEaQpPknKuMAGGSNFVJb0NtaOQSAocAOg1nLKh80v232Y8sJOHG63asSJoBa6bGwjIHftsqD~lEmVV4NkgNPybmvsD1SCbMQ2ExaCXFPVQV-yJhIAPN9MRVT9cSBT2GCq-vpMwdJ5Nf0iPR3M-Ak961JUwWXPYTL79toXCgxDX2~nZ5QFRV490YNnfB7LQu10G89wG8lzS9GWf2i-nk~~ez0Lq0dH7qQokFXdUkPc7bvSrxqkytrbd-h8O8AAAA\nzerobin.i2p=Jf64hlpW8ILKZGDe61ljHU5wzmUYwN2klOyhM2iR-8VkUEVgDZRuaToRlXIFW4k5J1ccTzGzMxR518BkCAE3jCFIyrbF0MjQDuXO5cwmqfBFWrIv72xgKDizu3HytE4vOF2M730rv8epSNPAJg6OpyXkf5UQW96kgL8SWcxWdTbKU-O8IpE3O01Oc6j0fp1E4wVOci7qIL8UEloNN~mulgka69MkR0uEtXWOXd6wvBjLNrZgdZi7XtT4QlDjx13jr7RGpZBJAUkk~8gLqgJwoUYhbfM7x564PIn3IlMXHK5AKRVxAbCQ5GkS8KdkvNL7FsQ~EiElGzZId4wenraHMHL0destUDmuwGdHKA7YdtovXD~OnaBvIbl36iuIduZnGKPEBD31hVLdJuVId9RND7lQy5BZJHQss5HSxMWTszAnWJDwmxqzMHHCiL6BMpZnkz8znwPDSkUwEs3P6-ba7mDKKt8EPCG0nM6l~BvPl2OKQIBhXIxJLOOavGyqmmYmAAAA\nzzz.i2p=GKapJ8koUcBj~jmQzHsTYxDg2tpfWj0xjQTzd8BhfC9c3OS5fwPBNajgF-eOD6eCjFTqTlorlh7Hnd8kXj1qblUGXT-tDoR9~YV8dmXl51cJn9MVTRrEqRWSJVXbUUz9t5Po6Xa247Vr0sJn27R4KoKP8QVj1GuH6dB3b6wTPbOamC3dkO18vkQkfZWUdRMDXk0d8AdjB0E0864nOT~J9Fpnd2pQE5uoFT6P0DqtQR2jsFvf9ME61aqLvKPPWpkgdn4z6Zkm-NJOcDz2Nv8Si7hli94E9SghMYRsdjU-knObKvxiagn84FIwcOpepxuG~kFXdD5NfsH0v6Uri3usE3XWD7Pw6P8qVYF39jUIq4OiNMwPnNYzy2N4mDMQdsdHO3LUVh~DEppOy9AAmEoHDjjJxt2BFBbGxfdpZCpENkwvmZeYUyNCCzASqTOOlNzdpne8cuesn3NDXIpNnqEE6Oe5Qm5YOJykrX~Vx~cFFT3QzDGkIjjxlFBsjUJyYkFjBQAEAAcAAA==#!action=adddest#date=1490103520#olddest=GKapJ8koUcBj~jmQzHsTYxDg2tpfWj0xjQTzd8BhfC9c3OS5fwPBNajgF-eOD6eCjFTqTlorlh7Hnd8kXj1qblUGXT-tDoR9~YV8dmXl51cJn9MVTRrEqRWSJVXbUUz9t5Po6Xa247Vr0sJn27R4KoKP8QVj1GuH6dB3b6wTPbOamC3dkO18vkQkfZWUdRMDXk0d8AdjB0E0864nOT~J9Fpnd2pQE5uoFT6P0DqtQR2jsFvf9ME61aqLvKPPWpkgdn4z6Zkm-NJOcDz2Nv8Si7hli94E9SghMYRsdjU-knObKvxiagn84FIwcOpepxuG~kFXdD5NfsH0v6Uri3usE3uSzpWS0EHmrlfoLr5uGGd9ZHwwCIcgfOATaPRMUEQxiK9q48PS0V3EXXO4-YLT0vIfk4xO~XqZpn8~PW1kFe2mQMHd7oO89yCk-3yizRG3UyFtI7-mO~eCI6-m1spYoigStgoupnC3G85gJkqEjMm49gUjbhfWKWI-6NwTj0ZnAAAA#oldsig=MbSvc9wsxSm37B65rUC~BCZzFsIJe0-CXCH8n97ZaMMizNUjeytgBQ==#sig=R2wREo~02liJmU4UGfVZr88XFMiHdYDXVfS~HtyxFxwYG~2o1guP~RocqmHBCE6yPg1Cm8m336d~jqijAVJzBA==".to_string();

        address_book.parse_and_merge(&mut addresses, hosts).await;

        assert_eq!(addresses.get(&String::from("tracker2.postman.i2p")), Some(&String::from("lnQ6yoBTxQuQU8EQ1FlF395ITIQF-HGJxUeFvzETLFnoczNjQvKDbtSB7aHhn853zjVXrJBgwlB9sO57KakBDaJ50lUZgVPhjlI19TgJ-CxyHhHSCeKx5JzURdEW-ucdONMynr-b2zwhsx8VQCJwCEkARvt21YkOyQDaB9IdV8aTAmP~PUJQxRwceaTMn96FcVenwdXqleE16fI8CVFOV18jbJKrhTOYpTtcZKV4l1wNYBDwKgwPx5c0kcrRzFyw5~bjuAKO~GJ5dR7BQsL7AwBoQUS4k1lwoYrG1kOIBeDD3XF8BWb6K3GOOoyjc1umYKpur3G~FxBuqtHAsDRICrsRuil8qK~whOvj8uNTv~ohZnTZHxTLgi~sDyo98BwJ-4Y4NMSuF4GLzcgLypcR1D1WY2tDqMKRYFVyLE~MTPVjRRgXfcKolykQ666~Go~A~~CNV4qc~zlO6F4bsUhVZDU7WJ7mxCAwqaMiJsL-NgIkb~SMHNxIzaE~oy0agHJMBQAEAAcAAA==")));

        assert_eq!(addresses.get(&String::from("psi.i2p")), Some(&String::from("a11l91etedRW5Kl2GhdDI9qiRBbDRAQY6TWJb8KlSc0P9WUrEviABAAltqDU1DFJrRhMAZg5i6rWGszkJrF-pWLQK9JOH33l4~mQjB8Hkt83l9qnNJPUlGlh9yIfBY40CQ0Ermy8gzjHLayUpypDJFv2V6rHLwxAQeaXJu8YXbyvCucEu9i6HVO49akXW9YSxcZEqxK04wZnjBqhHGlVbehleMqTx9nkd0pUpBZz~vIaG9matUSHinopEo6Wegml9FEz~FEaQpPknKuMAGGSNFVJb0NtaOQSAocAOg1nLKh80v232Y8sJOHG63asSJoBa6bGwjIHftsqD~lEmVV4NkgNPybmvsD1SCbMQ2ExaCXFPVQV-yJhIAPN9MRVT9cSBT2GCq-vpMwdJ5Nf0iPR3M-Ak961JUwWXPYTL79toXCgxDX2~nZ5QFRV490YNnfB7LQu10G89wG8lzS9GWf2i-nk~~ez0Lq0dH7qQokFXdUkPc7bvSrxqkytrbd-h8O8AAAA")));

        assert_eq!(addresses.get(&String::from("zerobin.i2p")), Some(&String::from("Jf64hlpW8ILKZGDe61ljHU5wzmUYwN2klOyhM2iR-8VkUEVgDZRuaToRlXIFW4k5J1ccTzGzMxR518BkCAE3jCFIyrbF0MjQDuXO5cwmqfBFWrIv72xgKDizu3HytE4vOF2M730rv8epSNPAJg6OpyXkf5UQW96kgL8SWcxWdTbKU-O8IpE3O01Oc6j0fp1E4wVOci7qIL8UEloNN~mulgka69MkR0uEtXWOXd6wvBjLNrZgdZi7XtT4QlDjx13jr7RGpZBJAUkk~8gLqgJwoUYhbfM7x564PIn3IlMXHK5AKRVxAbCQ5GkS8KdkvNL7FsQ~EiElGzZId4wenraHMHL0destUDmuwGdHKA7YdtovXD~OnaBvIbl36iuIduZnGKPEBD31hVLdJuVId9RND7lQy5BZJHQss5HSxMWTszAnWJDwmxqzMHHCiL6BMpZnkz8znwPDSkUwEs3P6-ba7mDKKt8EPCG0nM6l~BvPl2OKQIBhXIxJLOOavGyqmmYmAAAA")));

        assert_eq!(addresses.get(&String::from("zzz.i2p")), Some(&String::from("GKapJ8koUcBj~jmQzHsTYxDg2tpfWj0xjQTzd8BhfC9c3OS5fwPBNajgF-eOD6eCjFTqTlorlh7Hnd8kXj1qblUGXT-tDoR9~YV8dmXl51cJn9MVTRrEqRWSJVXbUUz9t5Po6Xa247Vr0sJn27R4KoKP8QVj1GuH6dB3b6wTPbOamC3dkO18vkQkfZWUdRMDXk0d8AdjB0E0864nOT~J9Fpnd2pQE5uoFT6P0DqtQR2jsFvf9ME61aqLvKPPWpkgdn4z6Zkm-NJOcDz2Nv8Si7hli94E9SghMYRsdjU-knObKvxiagn84FIwcOpepxuG~kFXdD5NfsH0v6Uri3usE3XWD7Pw6P8qVYF39jUIq4OiNMwPnNYzy2N4mDMQdsdHO3LUVh~DEppOy9AAmEoHDjjJxt2BFBbGxfdpZCpENkwvmZeYUyNCCzASqTOOlNzdpne8cuesn3NDXIpNnqEE6Oe5Qm5YOJykrX~Vx~cFFT3QzDGkIjjxlFBsjUJyYkFjBQAEAAcAAA==")));
    }
}
