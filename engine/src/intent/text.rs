//! BM25 text ranking over the history snapshot's past commit messages: a
//! query text scores the files the matching commits touched, indexed through
//! the one candidate file space (`rank::FileIndex`). Owner: EB.3.
