use std::net::Ipv4Addr;
use std::sync::Arc;

use actix::prelude::*;
use rustc_hash::FxHashMap;
use parking_lot::RwLock;
use serde::Deserialize;

use crate::chain::{Chain, LocateNode};
use crate::types::{NodeId, NodeLocation};

#[derive(Clone)]
pub struct Locator {
    client: reqwest::Client,
    cache: Arc<RwLock<FxHashMap<Ipv4Addr, Option<NodeLocation>>>>,
}

pub struct LocatorFactory {
    cache: Arc<RwLock<FxHashMap<Ipv4Addr, Option<NodeLocation>>>>,
}

impl LocatorFactory {
    pub fn new() -> Self {
        let mut cache = FxHashMap::default();

        // Default entry for localhost
        cache.insert(
            Ipv4Addr::new(127, 0, 0, 1),
            Some(NodeLocation { latitude: 52.5166667, longitude: 13.4, city: "Berlin".into() }),
        );

        LocatorFactory {
            cache: Arc::new(RwLock::new(cache)),
        }
    }

    pub fn create(&self) -> Locator {
        Locator {
            client: reqwest::Client::new(),
            cache: self.cache.clone(),
        }
    }
}

impl Actor for Locator {
    type Context = SyncContext<Self>;
}

#[derive(Message)]
#[rtype(result = "()")]
pub struct LocateRequest {
    pub ip: Ipv4Addr,
    pub nid: NodeId,
    pub chain: Addr<Chain>,
}

#[derive(Deserialize)]
pub struct IPApiLocate {
    city: Arc<str>,
    loc: Box<str>,
}

impl IPApiLocate {
    fn into_node_location(self) -> Option<NodeLocation> {
        let IPApiLocate { city, loc } = self;

        let mut loc = loc.split(",").map(|n| n.parse());

        let latitude = loc.next()?.ok()?;
        let longitude = loc.next()?.ok()?;

        // Guarantee that the iterator has been exhausted
        if loc.next().is_some() {
            return None;
        }

        Some(NodeLocation {
            latitude,
            longitude,
            city,
        })
    }
}

impl Handler<LocateRequest> for Locator {
    type Result = ();

    fn handle(&mut self, msg: LocateRequest, _: &mut <Self as Actor>::Context) {
        let LocateRequest { ip, nid, chain } = msg;

        if let Some(item) = self.cache.read().get(&ip) {
            if let Some(location) = item {
                return chain.do_send(LocateNode { nid, location: location.clone() });
            }

            return
        }

        let locator = self.clone();

        tokio::task::spawn(async move {
            let location = match locator.iplocate(ip).await {
                Ok(location) => location,
                Err(err) => return log::debug!("GET error for ip location: {:?}", err),
            };

            locator.cache.write().insert(ip, location.clone());

            if let Some(location) = location {
                chain.do_send(LocateNode { nid, location });
            }
        });
    }
}

impl Locator {
    async fn iplocate(&self, ip: Ipv4Addr) -> Result<Option<NodeLocation>, reqwest::Error> {
        let location = self.iplocate_ipapi_co(ip).await?;

        match location {
            Some(location) => Ok(Some(location)),
            None => self.iplocate_ipinfo_io(ip).await,
        }
    }

    async fn iplocate_ipapi_co(&self, ip: Ipv4Addr) -> Result<Option<NodeLocation>, reqwest::Error> {
        self.query(&format!("https://ipapi.co/{}/json", ip)).await
    }

    async fn iplocate_ipinfo_io(&self, ip: Ipv4Addr) -> Result<Option<NodeLocation>, reqwest::Error> {
        let location = self.query(&format!("https://ipinfo.io/{}/json", ip)).await?.and_then(|loc: IPApiLocate| {
            loc.into_node_location()
        });

        Ok(location)
    }

    async fn query<T>(&self, url: &str) -> Result<Option<T>, reqwest::Error>
    where
        for<'de> T: Deserialize<'de>,
    {
        match self.client.get(url).send().await?.json::<T>().await {
            Ok(result) => Ok(Some(result)),
            Err(err) => {
                log::debug!("JSON error for ip location: {:?}", err);
                Ok(None)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipapi_locate_to_node_location() {
        let ipapi = IPApiLocate {
            loc: "12.5,56.25".into(),
            city: "Foobar".into(),
        };

        let location = ipapi.into_node_location().unwrap();

        assert_eq!(location.latitude, 12.5);
        assert_eq!(location.longitude, 56.25);
        assert_eq!(&*location.city, "Foobar");
    }

    #[test]
    fn ipapi_locate_to_node_location_too_many() {
        let ipapi = IPApiLocate {
            loc: "12.5,56.25,1.0".into(),
            city: "Foobar".into(),
        };

        let location = ipapi.into_node_location();

        assert!(location.is_none());
    }
}
