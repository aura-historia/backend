# High-Level DynamoDB Structural Overview

- Single-Table-Design

## Primary Table `table_1`

![Primary Table](images/pt.png "Primary Table")

### Entities

#### Item

- PK is composite of `ShopId` and `ShopsProductId`
- Primary event-store - via EventId (UUIDv7) in SK
- Contains a materialized view with extra SK

#### Shop

- Multi-Partition per actual shop
  - PK is either:
    - the `ShopId`
    - any of the shops domains
  - SK is constant `details`
  - Partitions must be kept strictly in sync (`TransactWriteItems`)

#### User

- PK is `UserId` (Cognito-Sub)
- SK
  - Composite of `ShopId` and `ShopsProductId` for products on users watchlist
  - `SearchFilterId` for saved search-filters
  - Constant `details` for user-data (Cognito only acts as IDP)

## Local Secondary Indexes

### LSI1: `lsi1`

- Sparse (globally)
- SK is `lsi_sk`
- User
  - Uses it for sorting by creation-timestamp of watchlist-entries
  - Sets SK as that exact timestamp

## Global Secondary Indexes

### GSI1: `gsi1`

![Global Secondary Index 1](images/gsi1.png "Global Secondary Index 1")

- Sparse (globally)
- PK is `gsi1_pk`
- SK is `gsi1_sk`
- User
  - PK is `ProductId`
  - SK is `UserId`
  - Uses it to query all users having a specific product on their watchlist

### GSI2: `gsi2`

- Sparse (globally)
- PK is `gsi2_pk`
- SK is `gsi2_sk`
- Project keys-only
- Product
  - PK is `ShopSlugId + ProductSlugId`
  - SK is constant
  - Used for looking up `ShopId` and `ShopsProductId` from `pk` for given `ShopSlugId` and `ProductSlugId`
- Shop
  - PK is `ShopSlugId`
  - SK is constant
  - Used for looking up `ShopId` from `pk` for given `ShopSlugId`
