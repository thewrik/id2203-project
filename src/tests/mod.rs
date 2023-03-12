#[cfg(test)]
mod tests {
    use warp::http::StatusCode;
    use warp::test::request;
    use super::filters;

    #[tokio::test]
    async fn try_list() {
        let api = filters::list();

        let response = request()
            .method("GET")
            .path("/holodeck")
            .reply(&api)
            .await;

        assert_eq!(response.status(), StatusCode::ACCEPTED);
    }
}

mod filters{
    use warp::Filter;
    use super::handlers;

    pub fn list() ->  impl Filter<Extract = impl warp::Reply, Error = warp::Rejection> + Clone{ 
        warp::path!("holodeck")
            .and(warp::get())
            .and_then(handlers::handle_list)
    }
}

mod handlers{
    use warp::http::StatusCode;
    use std::convert::Infallible;

    pub async fn handle_list() -> Result<impl warp::Reply, Infallible> {
        // "Alright, alright, alright", Matthew said.
        Ok(StatusCode::ACCEPTED)
    }
}