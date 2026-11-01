import { NgModule } from '@angular/core';
import { APOLLO_OPTIONS } from 'apollo-angular';
import { HttpLink } from 'apollo-angular/http';
import { InMemoryCache } from '@apollo/client/core';

export function createApollo(httpLink: HttpLink) {
  return { link: httpLink.create({ uri: '/angular/graphql' }), cache: new InMemoryCache() };
}

@NgModule({
  providers: [{ provide: APOLLO_OPTIONS, useFactory: createApollo, deps: [HttpLink] }],
})
export class GraphQLModule {}
