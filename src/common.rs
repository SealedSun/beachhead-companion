
/// This macro is syntactic sugar for passing additional arguments to an error "conversion
/// constructor". The idea is that you define `From<(YourError, Additional, Args)>` (a conversion
/// from a tuple to an error) and then use this macro to supply the additional arguments.
/// ```
/// try_!(produced_your_error(&file_name), file_name.to_owned(), args.clone())
/// ```
/// The additional arguments are of course only evaluated when the guarded expression returns
/// an error.
#[macro_export]
macro_rules! try_ {
    ($expr:expr, $($details:expr),+) => (match $expr {
        ::std::result::Result::Ok(val) => val,
        ::std::result::Result::Err(err) => {
            return ::std::result::Result::Err(
                ::std::convert::From::from((err, ($($details),+) )))
        }
    })
}
