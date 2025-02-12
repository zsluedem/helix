use std::{sync::Arc, time::Duration};

use axum::{
    error_handling::HandleErrorLayer,
    http::StatusCode,
    routing::{get, post},
    BoxError, Extension, Router,
};
use helix_common::{chain_info::ChainInfo, ConstraintsApiConfig, RelayConfig, Route};
use tokio::sync::mpsc::{channel, Receiver, Sender};

use helix_beacon_client::{
    mock_block_broadcaster::MockBlockBroadcaster, mock_multi_beacon_client::MockMultiBeaconClient,
    BlockBroadcaster,
};
use helix_common::{signing::RelaySigningContext, ValidatorPreferences};
use helix_database::MockDatabaseService;
use helix_datastore::MockAuctioneer;
use helix_housekeeper::ChainUpdate;
use tower::{buffer::BufferLayer, limit::RateLimitLayer, timeout::TimeoutLayer, ServiceBuilder};
use tower_http::limit::RequestBodyLimitLayer;

use crate::{
    builder::{
        api::{BuilderApi, MAX_PAYLOAD_LENGTH},
        mock_simulator::MockSimulator,
    },
    constraints::api::{ConstraintsApi, ConstraintsHandle},
    gossiper::{mock_gossiper::MockGossiper, types::GossipedMessage},
    proposer::{
        api::{ProposerApi, MAX_BLINDED_BLOCK_LENGTH, MAX_VAL_REGISTRATIONS_LENGTH},
        PATH_GET_HEADER, PATH_GET_HEADER_WITH_PROOFS, PATH_GET_PAYLOAD, PATH_PROPOSER_API,
        PATH_REGISTER_VALIDATORS, PATH_STATUS,
    },
    relay_data::{
        DataApi, PATH_BUILDER_BIDS_RECEIVED, PATH_DATA_API, PATH_PROPOSER_PAYLOAD_DELIVERED,
        PATH_VALIDATOR_REGISTRATION,
    },
};

pub fn app() -> Router {
    let (slot_update_sender, _slot_update_receiver) = channel::<Sender<ChainUpdate>>(32);
    let (_gossip_sender, gossip_receiver) = channel::<GossipedMessage>(32);

    let api_service = Arc::new(ProposerApi::<
        MockAuctioneer,
        MockDatabaseService,
        MockMultiBeaconClient,
        MockGossiper,
    >::new(
        Arc::new(MockAuctioneer::default()),
        Arc::new(MockDatabaseService::default()),
        Arc::new(MockGossiper::new().unwrap()),
        vec![Arc::new(BlockBroadcaster::Mock(MockBlockBroadcaster::default()))],
        Arc::new(MockMultiBeaconClient::default()),
        Arc::new(ChainInfo::for_mainnet()),
        slot_update_sender,
        Arc::new(ValidatorPreferences::default()),
        gossip_receiver,
        Default::default(),
    ));

    let data_api = Arc::new(DataApi::<MockDatabaseService>::new(
        Arc::new(ValidatorPreferences::default()),
        Arc::new(MockDatabaseService::default()),
    ));

    Router::new()
        .route(
            &format!("{PATH_PROPOSER_API}{PATH_STATUS}"),
            get(ProposerApi::<
                MockAuctioneer,
                MockDatabaseService,
                MockMultiBeaconClient,
                MockGossiper,
            >::status),
        )
        .route(
            &format!("{PATH_PROPOSER_API}{PATH_REGISTER_VALIDATORS}"),
            post(
                ProposerApi::<
                    MockAuctioneer,
                    MockDatabaseService,
                    MockMultiBeaconClient,
                    MockGossiper,
                >::register_validators,
            ),
        )
        .route(
            &format!("{PATH_PROPOSER_API}{PATH_GET_HEADER}"),
            get(ProposerApi::<
                MockAuctioneer,
                MockDatabaseService,
                MockMultiBeaconClient,
                MockGossiper,
            >::get_header),
        )
        .route(
            &format!("{PATH_PROPOSER_API}{PATH_GET_HEADER_WITH_PROOFS}"),
            get(ProposerApi::<
                MockAuctioneer,
                MockDatabaseService,
                MockMultiBeaconClient,
                MockGossiper,
            >::get_header_with_proofs),
        )
        .route(
            &format!("{PATH_PROPOSER_API}{PATH_GET_PAYLOAD}"),
            post(
                ProposerApi::<
                    MockAuctioneer,
                    MockDatabaseService,
                    MockMultiBeaconClient,
                    MockGossiper,
                >::get_payload,
            ),
        )
        .route(
            &format!("{PATH_DATA_API}{PATH_PROPOSER_PAYLOAD_DELIVERED}"),
            get(DataApi::<MockDatabaseService>::proposer_payload_delivered),
        )
        .route(
            &format!("{PATH_DATA_API}{PATH_BUILDER_BIDS_RECEIVED}"),
            get(DataApi::<MockDatabaseService>::builder_bids_received),
        )
        .route(
            &format!("{PATH_DATA_API}{PATH_VALIDATOR_REGISTRATION}"),
            get(DataApi::<MockDatabaseService>::validator_registration),
        )
        .layer(Extension(api_service))
        .layer(Extension(data_api))
}

#[allow(clippy::type_complexity)]
pub fn builder_api_app() -> (
    Router,
    Arc<BuilderApi<MockAuctioneer, MockDatabaseService, MockSimulator, MockGossiper>>,
    Receiver<Sender<ChainUpdate>>,
    ConstraintsHandle,
) {
    let (slot_update_sender, slot_update_receiver) = channel::<Sender<ChainUpdate>>(32);
    let (_gossip_sender, gossip_receiver) = tokio::sync::mpsc::channel(10);

    let (builder_api_service, handler) =
        BuilderApi::<MockAuctioneer, MockDatabaseService, MockSimulator, MockGossiper>::new(
            Arc::new(MockAuctioneer::default()),
            Arc::new(MockDatabaseService::default()),
            Arc::new(ChainInfo::for_mainnet()),
            MockSimulator::default(),
            Arc::new(MockGossiper::new().unwrap()),
            Arc::new(RelaySigningContext::default()),
            RelayConfig::default(),
            slot_update_sender.clone(),
            gossip_receiver,
        );
    let builder_api_service = Arc::new(builder_api_service);

    let mut router = Router::new()
        .route(
            &Route::GetValidators.path(),
            get(BuilderApi::<MockAuctioneer, MockDatabaseService, MockSimulator, MockGossiper>::get_validators),
        )
        .route(
            &Route::SubmitBlock.path(),
            post(BuilderApi::<MockAuctioneer, MockDatabaseService, MockSimulator, MockGossiper>::submit_block),
        )
        .route(
            &Route::GetTopBid.path(),
            get(BuilderApi::<MockAuctioneer, MockDatabaseService, MockSimulator, MockGossiper>::get_top_bid),
        )
        .route(&Route::GetBuilderConstraintsStream.path(),
            get(BuilderApi::<MockAuctioneer, MockDatabaseService, MockSimulator, MockGossiper>::constraints_stream),
        )
        .route(&Route::GetBuilderConstraints.path(),
            get(BuilderApi::<MockAuctioneer, MockDatabaseService, MockSimulator, MockGossiper>::constraints),
        )
        .layer(RequestBodyLimitLayer::new(MAX_PAYLOAD_LENGTH))
        .layer(Extension(builder_api_service.clone()));

    // Add Timeout-Layer
    // Add Rate-Limit-Layer (buffered so we can clone the service)
    // Add Error-handling layer
    router = router.layer(
        ServiceBuilder::new()
            .layer(HandleErrorLayer::new(|_: BoxError| async { StatusCode::REQUEST_TIMEOUT }))
            .layer(TimeoutLayer::new(Duration::from_secs(5)))
            .layer(HandleErrorLayer::new(|err: BoxError| async move {
                (StatusCode::INTERNAL_SERVER_ERROR, format!("Unhandled error: {}", err))
            }))
            .layer(BufferLayer::new(4096))
            .layer(RateLimitLayer::new(100, Duration::from_secs(1))),
    );

    (router, builder_api_service, slot_update_receiver, handler)
}

#[allow(clippy::type_complexity)]
pub fn proposer_api_app() -> (
    Router,
    Arc<ProposerApi<MockAuctioneer, MockDatabaseService, MockMultiBeaconClient, MockGossiper>>,
    Receiver<Sender<ChainUpdate>>,
    Arc<MockAuctioneer>,
) {
    let (slot_update_sender, slot_update_receiver) = channel::<Sender<ChainUpdate>>(32);
    let (_gossip_sender, gossip_receiver) = channel::<GossipedMessage>(32);
    let auctioneer = Arc::new(MockAuctioneer::default());
    let proposer_api_service = Arc::new(ProposerApi::<
        MockAuctioneer,
        MockDatabaseService,
        MockMultiBeaconClient,
        MockGossiper,
    >::new(
        auctioneer.clone(),
        Arc::new(MockDatabaseService::default()),
        Arc::new(MockGossiper::new().unwrap()),
        vec![Arc::new(BlockBroadcaster::Mock(MockBlockBroadcaster::default()))],
        Arc::new(MockMultiBeaconClient::default()),
        Arc::new(ChainInfo::for_mainnet()),
        slot_update_sender.clone(),
        Arc::new(ValidatorPreferences::default()),
        gossip_receiver,
        Default::default(),
    ));

    let router = Router::new()
        .route(
            &format!("{PATH_PROPOSER_API}{PATH_GET_HEADER}"),
            get(ProposerApi::<
                MockAuctioneer,
                MockDatabaseService,
                MockMultiBeaconClient,
                MockGossiper,
            >::get_header),
        )
        .route(
            &format!("{PATH_PROPOSER_API}{PATH_GET_HEADER_WITH_PROOFS}"),
            get(ProposerApi::<
                MockAuctioneer,
                MockDatabaseService,
                MockMultiBeaconClient,
                MockGossiper,
            >::get_header_with_proofs),
        )
        .route(
            &format!("{PATH_PROPOSER_API}{PATH_GET_PAYLOAD}"),
            post(
                ProposerApi::<
                    MockAuctioneer,
                    MockDatabaseService,
                    MockMultiBeaconClient,
                    MockGossiper,
                >::get_payload,
            ),
        )
        .layer(RequestBodyLimitLayer::new(MAX_BLINDED_BLOCK_LENGTH))
        .route(
            &format!("{PATH_PROPOSER_API}{PATH_REGISTER_VALIDATORS}"),
            post(
                ProposerApi::<
                    MockAuctioneer,
                    MockDatabaseService,
                    MockMultiBeaconClient,
                    MockGossiper,
                >::register_validators,
            ),
        )
        .layer(RequestBodyLimitLayer::new(MAX_VAL_REGISTRATIONS_LENGTH))
        .layer(Extension(proposer_api_service.clone()));

    (router, proposer_api_service, slot_update_receiver, auctioneer)
}

pub fn data_api_app() -> (Router, Arc<DataApi<MockDatabaseService>>, Arc<MockDatabaseService>) {
    let mock_database = Arc::new(MockDatabaseService::default());
    let proposer_api_service = Arc::new(DataApi::<MockDatabaseService>::new(
        Arc::new(ValidatorPreferences::default()),
        mock_database.clone(),
    ));

    let router = Router::new()
        .route(
            &format!("{PATH_DATA_API}{PATH_PROPOSER_PAYLOAD_DELIVERED}"),
            get(DataApi::<MockDatabaseService>::proposer_payload_delivered),
        )
        .route(
            &format!("{PATH_DATA_API}{PATH_BUILDER_BIDS_RECEIVED}"),
            get(DataApi::<MockDatabaseService>::builder_bids_received),
        )
        .route(
            &format!("{PATH_DATA_API}{PATH_VALIDATOR_REGISTRATION}"),
            get(DataApi::<MockDatabaseService>::validator_registration),
        )
        .layer(Extension(proposer_api_service.clone()));

    (router, proposer_api_service, mock_database)
}

#[allow(clippy::type_complexity)]
pub fn constraints_api_app() -> (
    Router,
    Arc<ConstraintsApi<MockAuctioneer>>,
    Arc<BuilderApi<MockAuctioneer, MockDatabaseService, MockSimulator, MockGossiper>>,
    Receiver<Sender<ChainUpdate>>,
) {
    let auctioneer = Arc::new(MockAuctioneer::default());
    let database = Arc::new(MockDatabaseService::default());

    let (slot_update_sender, slot_update_receiver) = channel::<Sender<ChainUpdate>>(32);
    let (_gossip_sender, gossip_receiver) = tokio::sync::mpsc::channel(10);

    let (builder_api_service, handler) =
        BuilderApi::<MockAuctioneer, MockDatabaseService, MockSimulator, MockGossiper>::new(
            auctioneer.clone(),
            database.clone(),
            Arc::new(ChainInfo::for_mainnet()),
            MockSimulator::default(),
            Arc::new(MockGossiper::new().unwrap()),
            Arc::new(RelaySigningContext::default()),
            RelayConfig::default(),
            slot_update_sender.clone(),
            gossip_receiver,
        );
    let builder_api_service = Arc::new(builder_api_service);

    let constraints_api_service = Arc::new(ConstraintsApi::<MockAuctioneer>::new(
        auctioneer.clone(),
        Arc::new(ChainInfo::for_mainnet()),
        slot_update_sender,
        handler,
        Arc::new(ConstraintsApiConfig::default()),
    ));

    let router = Router::new()
        .route(
            &Route::GetValidators.path(),
            get(BuilderApi::<MockAuctioneer, MockDatabaseService, MockSimulator, MockGossiper>::get_validators),
        )
        .route(
            &Route::SubmitBlock.path(),
            post(BuilderApi::<MockAuctioneer, MockDatabaseService, MockSimulator, MockGossiper>::submit_block),
        )
        .route(
            &Route::GetTopBid.path(),
            get(BuilderApi::<MockAuctioneer, MockDatabaseService, MockSimulator, MockGossiper>::get_top_bid),
        )
        .route(
            &Route::GetBuilderConstraints.path(),
            get(BuilderApi::<MockAuctioneer, MockDatabaseService, MockSimulator, MockGossiper>::constraints),
        )
        .route(
            &Route::GetBuilderConstraintsStream.path(),
            get(BuilderApi::<MockAuctioneer, MockDatabaseService, MockSimulator, MockGossiper>::constraints_stream),
        )
        .route(
            &Route::GetBuilderDelegations.path(),
            get(BuilderApi::<MockAuctioneer, MockDatabaseService, MockSimulator, MockGossiper>::delegations),
        )
        .route(
            &Route::SubmitBuilderConstraints.path(),
            post(ConstraintsApi::<MockAuctioneer>::submit_constraints),
        )
        .route(
            &Route::DelegateSubmissionRights.path(),
            post(ConstraintsApi::<MockAuctioneer>::delegate),
        )
        .route(
            &Route::RevokeSubmissionRights.path(),
            post(ConstraintsApi::<MockAuctioneer>::revoke),
        )
        .layer(RequestBodyLimitLayer::new(MAX_PAYLOAD_LENGTH))
        .layer(Extension(builder_api_service.clone()))
        .layer(Extension(constraints_api_service.clone()));

    (router, constraints_api_service, builder_api_service, slot_update_receiver)
}
